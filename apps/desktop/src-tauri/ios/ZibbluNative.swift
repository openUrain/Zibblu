import Darwin
import Foundation
import ImageIO
import Network
import PhotosUI
import Security
import UIKit
import UniformTypeIdentifiers
import dnssd

private let serviceType = "_zibblu-private._udp"

private final class NativeState {
    private let lock = NSLock()
    private var wifi: UInt64 = 0
    private var discovery: (id: UInt64, status: Int32, data: Data) = (0, 0, Data())

    func setInterface(_ index: UInt32) {
        lock.lock()
        wifi = ((wifi >> 32) &+ 1) << 32 | UInt64(index)
        lock.unlock()
    }

    func wifiState() -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return wifi
    }

    func startDiscovery(_ id: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        discovery = (id, 0, Data())
    }

    func updateDiscovery(_ id: UInt64, status: Int32, peers: [[Any]]) {
        let payload = (try? JSONSerialization.data(withJSONObject: peers)) ?? Data()
        lock.lock()
        defer { lock.unlock() }
        if discovery.id == id {
            discovery = (id, status, payload)
        }
    }

    func status(_ id: UInt64) -> Int32 {
        lock.lock()
        defer { lock.unlock() }
        return discovery.id == id ? discovery.status : 3
    }

    func snapshot(_ id: UInt64) -> Data {
        lock.lock()
        defer { lock.unlock() }
        return discovery.id == id && discovery.status == 1 ? discovery.data : Data()
    }
}

private let native = NativeState()

private func appWindow() -> UIWindow? {
    let windows = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows)
    return windows.first(where: \.isKeyWindow) ?? UIApplication.shared.windows.first(where: \.isKeyWindow)
}

private final class Lifecycle {
    static let shared = Lifecycle()
    private let monitor = NWPathMonitor(requiredInterfaceType: .wifi)
    private var foreground = false
    private var index: UInt32 = 0
    private var previous: UInt32 = 0
    private var cover: UIView?

    func install(_ changed: @escaping @convention(c) (Bool) -> Void) {
        foreground = UIApplication.shared.applicationState == .active
        let center = NotificationCenter.default
        center.addObserver(forName: UIApplication.willResignActiveNotification, object: nil, queue: .main) { _ in
            guard self.cover == nil, let window = appWindow() else { return }
            let cover = UIView(frame: window.bounds)
            cover.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            cover.backgroundColor = UIColor(red: 0.96, green: 0.96, blue: 0.97, alpha: 1)
            window.addSubview(cover)
            self.cover = cover
        }
        center.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { _ in
            self.foreground = false
            self.update(changed)
        }
        center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
            self.foreground = true
            self.update(changed)
            self.cover?.removeFromSuperview()
            self.cover = nil
        }
        monitor.pathUpdateHandler = { path in
            let wifi = path.availableInterfaces.first { $0.type == .wifi }
            self.index = path.status == .satisfied ? UInt32(wifi?.index ?? 0) : 0
            self.update(changed, pathChanged: true)
        }
        monitor.start(queue: .main)
    }

    private func update(_ changed: @convention(c) (Bool) -> Void, pathChanged: Bool = false) {
        let current = foreground ? index : 0
        guard current != previous || (pathChanged && current != 0) else { return }
        previous = current
        native.setInterface(current)
        changed(current != 0)
    }
}

@_cdecl("zibblu_ios_install")
func zibbluInstall(_ changed: @escaping @convention(c) (Bool) -> Void) {
    DispatchQueue.main.async { Lifecycle.shared.install(changed) }
}

@_cdecl("zibblu_wifi_state")
func zibbluWifiState() -> UInt64 {
    native.wifiState()
}

@_cdecl("zibblu_ios_data_dir")
func zibbluDataDirectory(_ buffer: UnsafeMutablePointer<UInt8>, _ capacity: Int) -> Int {
    guard let url = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return 0 }
    let path = Data(url.appendingPathComponent("Zibblu", isDirectory: true).path.utf8)
    guard path.count <= capacity else { return 0 }
    path.copyBytes(to: buffer, count: path.count)
    return path.count
}

private final class Bonjour: NSObject, NetServiceDelegate {
    static var current: Bonjour?
    let id: UInt64
    private let own: NetService
    private let browser: NWBrowser
    private var published = false
    private var publicationFailed = false
    private var browseStatus: Int32 = 0
    private var resolving: [String: NetService] = [:]
    private var peers: [String: [Any]] = [:]

    init(id: UInt64, name: String, tag: String, port: UInt16) {
        self.id = id
        own = NetService(domain: "local.", type: serviceType + ".", name: name, port: Int32(port))
        let parameters = NWParameters.udp
        parameters.requiredInterfaceType = .wifi
        parameters.includePeerToPeer = false
        browser = NWBrowser(for: .bonjourWithTXTRecord(type: serviceType, domain: "local."), using: parameters)
        super.init()
        own.delegate = self
        own.setTXTRecord(NetService.data(fromTXTRecord: ["account": Data(tag.utf8)]))
        browser.stateUpdateHandler = { [weak self] state in self?.updateState(state) }
        browser.browseResultsChangedHandler = { [weak self] results, changes in
            for change in changes {
                if case .changed(old: let old, new: _, flags: _) = change,
                   case let .service(name, _, _, _) = old.endpoint {
                    self?.remove(name)
                }
            }
            self?.updateResults(results)
        }
        browser.start(queue: .main)
        own.publish(options: .noAutoRename)
    }

    func stop() {
        browser.cancel()
        own.stop()
        for service in resolving.values { service.stop() }
        resolving.removeAll()
        peers.removeAll()
    }

    private func updateState(_ state: NWBrowser.State) {
        switch state {
        case .ready:
            browseStatus = 1
        case .waiting(let error), .failed(let error):
            if case .dns(let code) = error, code == kDNSServiceErr_PolicyDenied {
                browseStatus = 2
            } else {
                browseStatus = 3
            }
        case .cancelled:
            browseStatus = 3
        default:
            browseStatus = 0
        }
        updateSnapshot()
    }

    private func updateSnapshot() {
        let status: Int32 = browseStatus == 2 ? 2
            : (publicationFailed ? 3 : (browseStatus == 1 && !published ? 0 : browseStatus))
        native.updateDiscovery(id, status: status, peers: status == 1 ? Array(peers.values) : [])
    }

    private func remove(_ name: String) {
        resolving.removeValue(forKey: name)?.stop()
        peers.removeValue(forKey: name)
    }

    private func updateResults(_ results: Set<NWBrowser.Result>) {
        let detected = results.compactMap { result -> String? in
            guard case let .service(name, _, _, _) = result.endpoint,
                  name != own.name, name.utf8.count == 32, name.allSatisfy(\.isHexDigit) else { return nil }
            return name
        }
        let names = Set(detected.sorted().prefix(512))
        for name in Set(resolving.keys).union(peers.keys).subtracting(names) { remove(name) }
        for name in names where resolving[name] == nil && peers[name] == nil {
            let service = NetService(domain: "local.", type: serviceType + ".", name: name)
            service.delegate = self
            resolving[name] = service
            service.resolve(withTimeout: 4)
        }
        updateSnapshot()
    }

    func netServiceDidPublish(_ sender: NetService) {
        published = true
        updateSnapshot()
    }

    func netService(_ sender: NetService, didNotPublish error: [String: NSNumber]) {
        publicationFailed = true
        updateSnapshot()
    }

    func netService(_ sender: NetService, didNotResolve error: [String: NSNumber]) {
        if resolving[sender.name] === sender { remove(sender.name) }
    }

    func netServiceDidResolveAddress(_ sender: NetService) {
        guard resolving[sender.name] === sender else { return }
        resolving.removeValue(forKey: sender.name)
        defer { sender.stop() }
        guard let record = sender.txtRecordData(),
              let tagData = NetService.dictionary(fromTXTRecord: record)["account"],
              let tag = String(data: tagData, encoding: .utf8),
              tag.utf8.count == 64, tag.allSatisfy(\.isHexDigit),
              (1...65535).contains(sender.port) else { return }
        let addresses = (sender.addresses ?? []).prefix(8).compactMap { data -> String? in
            guard data.count >= MemoryLayout<sockaddr_in>.size else { return nil }
            var address = sockaddr_in()
            _ = withUnsafeMutableBytes(of: &address) { data.copyBytes(to: $0) }
            guard address.sin_family == sa_family_t(AF_INET), let ip = inet_ntoa(address.sin_addr) else { return nil }
            return String(cString: ip)
        }
        if !addresses.isEmpty {
            peers[sender.name] = [sender.name, tag, sender.port, addresses]
            updateSnapshot()
        }
    }
}

@_cdecl("zibblu_bonjour_start")
func zibbluBonjourStart(_ id: UInt64, _ name: UnsafePointer<CChar>, _ tag: UnsafePointer<CChar>, _ port: UInt16, _ wifi: UInt64) {
    let name = String(cString: name)
    let tag = String(cString: tag)
    native.startDiscovery(id)
    DispatchQueue.main.async {
        guard native.wifiState() == wifi else { return }
        Bonjour.current?.stop()
        Bonjour.current = Bonjour(id: id, name: name, tag: tag, port: port)
    }
}

@_cdecl("zibblu_bonjour_stop")
func zibbluBonjourStop(_ id: UInt64) {
    native.updateDiscovery(id, status: 3, peers: [])
    DispatchQueue.main.async {
        guard Bonjour.current?.id == id else { return }
        Bonjour.current?.stop()
        Bonjour.current = nil
    }
}

@_cdecl("zibblu_bonjour_status")
func zibbluBonjourStatus(_ id: UInt64) -> Int32 {
    native.status(id)
}

@_cdecl("zibblu_bonjour_snapshot")
func zibbluBonjourSnapshot(_ id: UInt64, _ buffer: UnsafeMutablePointer<UInt8>, _ capacity: Int) -> Int {
    let data = native.snapshot(id)
    guard data.count <= capacity else { return 0 }
    data.copyBytes(to: buffer, count: data.count)
    return data.count
}

private func keychainQuery(_ account: UnsafePointer<CChar>) -> [String: Any] {
    [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "com.zibblu.ios.credentials",
        kSecAttrAccount as String: String(cString: account)
    ]
}

@_cdecl("zibblu_keychain_write")
func zibbluKeychainWrite(_ account: UnsafePointer<CChar>, _ bytes: UnsafePointer<UInt8>, _ length: Int) -> Bool {
    let query = keychainQuery(account)
    let data = Data(bytes: bytes, count: length)
    let attributes: [String: Any] = [
        kSecValueData as String: data,
        kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly
    ]
    let updated = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    if updated == errSecItemNotFound {
        return SecItemAdd(query.merging(attributes) { _, new in new } as CFDictionary, nil) == errSecSuccess
    }
    return updated == errSecSuccess
}

@_cdecl("zibblu_keychain_read")
func zibbluKeychainRead(_ account: UnsafePointer<CChar>, _ buffer: UnsafeMutablePointer<UInt8>, _ capacity: Int) -> Int {
    var query = keychainQuery(account)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &item)
    if status == errSecItemNotFound { return -1 }
    guard status == errSecSuccess, let data = item as? Data, data.count <= capacity else { return -2 }
    data.copyBytes(to: buffer, count: data.count)
    return data.count
}

@_cdecl("zibblu_keychain_delete")
func zibbluKeychainDelete(_ account: UnsafePointer<CChar>) -> Bool {
    let status = SecItemDelete(keychainQuery(account) as CFDictionary)
    return status == errSecSuccess || status == errSecItemNotFound
}

@_cdecl("zibblu_ios_share")
func zibbluShare(_ path: UnsafePointer<CChar>) -> Bool {
    let url = URL(fileURLWithPath: String(cString: path))
    let present = {
        guard UIApplication.shared.applicationState == .active,
              var controller = appWindow()?.rootViewController else { return false }
        while let shown = controller.presentedViewController { controller = shown }
        let sheet = UIActivityViewController(activityItems: [url], applicationActivities: nil)
        if let popover = sheet.popoverPresentationController {
            popover.sourceView = controller.view
            popover.sourceRect = CGRect(x: controller.view.bounds.midX, y: controller.view.bounds.midY, width: 1, height: 1)
        }
        controller.present(sheet, animated: true)
        return true
    }
    return Thread.isMainThread ? present() : DispatchQueue.main.sync(execute: present)
}

@_cdecl("zibblu_ios_copy_text")
func zibbluCopyText(_ text: UnsafePointer<CChar>) -> Bool {
    let value = String(cString: text)
    let copy = {
        guard UIApplication.shared.applicationState == .active else { return false }
        UIPasteboard.general.string = value
        return true
    }
    return Thread.isMainThread ? copy() : DispatchQueue.main.sync(execute: copy)
}

typealias PhotoCallback = @convention(c) (UnsafeMutableRawPointer, Int32, UnsafePointer<UInt8>?, Int) -> Void

private final class PhotoPicker: NSObject, PHPickerViewControllerDelegate {
    static var active: PhotoPicker?
    let context: UnsafeMutableRawPointer
    let callback: PhotoCallback

    init(_ context: UnsafeMutableRawPointer, _ callback: @escaping PhotoCallback) {
        self.context = context
        self.callback = callback
    }

    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        picker.dismiss(animated: true)
        guard let item = results.first?.itemProvider else { return finish(0) }
        guard let type = item.registeredTypeIdentifiers.first(where: { UTType($0)?.conforms(to: .image) == true })
            else { return finish(-1) }
        item.loadFileRepresentation(forTypeIdentifier: type) { url, _ in
            let data = url.flatMap(Self.preview)
            DispatchQueue.main.async {
                if let data = data { self.finish(1, data) }
                else { self.finish(-1) }
            }
        }
    }

    private static func preview(_ url: URL) -> Data? {
        let limit = 16 * 1024 * 1024
        guard let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize, size <= limit,
              let source = CGImageSourceCreateWithURL(url as CFURL, nil)
            else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: 2048,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
            else { return nil }
        return UIImage(cgImage: image).pngData()
    }

    private func finish(_ status: Int32, _ data: Data = Data()) {
        Self.active = nil
        data.withUnsafeBytes { callback(context, status, $0.bindMemory(to: UInt8.self).baseAddress, $0.count) }
    }
}

@_cdecl("zibblu_ios_pick_photo")
func zibbluPickPhoto(_ context: UnsafeMutableRawPointer, _ callback: @escaping PhotoCallback) {
    DispatchQueue.main.async {
        guard PhotoPicker.active == nil, UIApplication.shared.applicationState == .active,
              var controller = appWindow()?.rootViewController else { return callback(context, -1, nil, 0) }
        while let shown = controller.presentedViewController { controller = shown }
        var config = PHPickerConfiguration()
        config.filter = .images
        config.selectionLimit = 1
        let picker = PHPickerViewController(configuration: config)
        let delegate = PhotoPicker(context, callback)
        PhotoPicker.active = delegate
        picker.delegate = delegate
        picker.isModalInPresentation = true
        controller.present(picker, animated: true)
    }
}
