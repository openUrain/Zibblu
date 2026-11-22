import { FloatingField } from "./floating-field";
import { ChevronLeft, ChevronRight, MessageCircle, MoreHorizontal, QrCode, UserRoundMinus } from "lucide-react";
import { useContext, useEffect, useId, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import type { FormEvent, ReactNode } from "react";
import { api, errorMessage } from "./api";
import type { Profile, Session, User } from "./api";
import { People, PersonAvatar, displayName, placeDialog } from "./common";
import { actions, t } from "./locale";
import { ios, phone, useIosBack } from "./ios";
import { QrContent } from "./qr";
import { ProfileImageEditor } from "./profile-image";
import { useMenu, useMenuAnchor } from "./context-menu";
import { ProfileCache } from "./workspace-cache";
import { useWorkspaceImage, WorkspaceImages } from "./workspace-image";

type ImageField = "avatar" | "cover";
type Page = ImageField | "display_name" | "bio" | "notes" | "greeting" | "qr" | "remove";

const labels: Record<Page, string> = {
  avatar: t("Avatar", "头像"), cover: t("Cover", "封面"), display_name: t("Name", "名称"),
  bio: t("Bio", "简介"), notes: t("Contact notes", "备注与资料"), greeting: t("Contact request", "添加联系人"),
  qr: t("My QR code", "我的二维码"),
  remove: t("Remove contact", "删除联系人"),
};

function Row({ label, children, choose, disabled, entry }: {
  label: string; children?: ReactNode; choose: () => void; disabled?: boolean; entry?: Page | "details";
}) {
  return <button className="profile-row" disabled={disabled} onClick={choose} data-profile-entry={entry}>
    <span>{label}</span><span className="profile-value">{children}</span><ChevronRight size={17} aria-hidden />
  </button>;
}

export function ProfileView({ session, user, relationVersion, reconnecting, close, refresh, edited, message,
  popup, point, active = true, onContactUnavailable, cache, directoryVersion }: {
  session: Session; user: User; relationVersion: string; reconnecting: boolean;
  close: () => void; refresh: () => Promise<void>; edited: (id: string, patch: Partial<Profile>) => void;
  popup?: boolean; point?: { x: number; y: number }; message: () => Promise<void>;
  active?: boolean; onContactUnavailable?: (id: string) => void;
  cache: ProfileCache; directoryVersion: string;
}) {
  const { remarks, avatars } = useContext(People);
  const openMenu = useMenu();
  const menuAnchor = useMenuAnchor();
  const more = useRef<HTMLButtonElement>(null);
  const own = user.id === session.user.id;
  const [temporaryCache] = useState(() => new ProfileCache());
  const shared = own || relationVersion === "contact";
  const source = shared ? cache : temporaryCache;
  const cacheKey = user.id + "/" + (own ? "own" : relationVersion);
  const current = useSyncExternalStore(source.subscribe, () => source.get(cacheKey));
  const [page, setPage] = useState<Page>();
  const [photo, setPhoto] = useState<ArrayBuffer>();
  const [picking, setPicking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saveError, setSaveError] = useState("");
  const [loadError, setLoadError] = useState("");
  const [saved, setSaved] = useState(false);
  const [retry, setRetry] = useState(0);
  const wasReconnecting = useRef(reconnecting);
  const backButton = useRef<HTMLButtonElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const heading = useId();
  const outsidePress = useRef(false);
  const returnEntry = useRef<Page | "details">(undefined);
  const directoryDetail = !!onContactUnavailable && !popup;
  const profile = current && current.expires_at > Date.now() ? current.profile : undefined;
  const contact = profile?.relation === "contact";
  const contactLayout = contact || (shared && !own && !profile && relationVersion === "contact");

  useEffect(() => {
    const reconnected = wasReconnecting.current && !reconnecting;
    wasReconnecting.current = reconnecting;
    if (!active) {
      setPage(undefined);
      setSaveError("");
      setSaved(false);
      return;
    }
    let mounted = true;
    async function read(force = false) {
      try {
        await source.load(cacheKey, directoryVersion,
          () => shared ? api.cachedProfile(user.id) : Promise.resolve(null),
          () => api.profile(user.id), force);
        if (!mounted) return;
        const current = source.get(cacheKey);
        if (onContactUnavailable && current && current.profile.relation !== "contact") onContactUnavailable(user.id);
        setLoadError("");
      } catch (error) {
        if (!mounted) return;
        const status = typeof error === "object" && error && "status" in error ? error.status : undefined;
        if (status === 401 || status === 403 || status === 404) onContactUnavailable?.(user.id);
        setLoadError(errorMessage(error));
      }
    }
    void read(reconnected || retry > 0);
    const interval = window.setInterval(() => void read(), 60_000);
    return () => { mounted = false; window.clearInterval(interval); };
  }, [user.id, source, cacheKey, directoryVersion, retry, reconnecting, active, onContactUnavailable, shared]);

  useEffect(() => {
    if (!current) return;
    const timer = window.setTimeout(() => {
      setRetry(value => value + 1);
    }, Math.max(0, current.expires_at - Date.now()));
    return () => window.clearTimeout(timer);
  }, [current]);

  useEffect(() => {
    if (!popup && !directoryDetail && page === undefined) backButton.current?.focus({ preventScroll: true });
  }, [page, popup, directoryDetail]);

  useLayoutEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => element?.close();
  }, []);

  useLayoutEffect(() => {
    if (!popup) return;
    const entry = returnEntry.current;
    returnEntry.current = undefined;
    if (entry) {
      const trigger = dialog.current?.querySelector<HTMLButtonElement>(`[data-profile-entry="${entry}"]:enabled`);
      (trigger ?? backButton.current)?.focus();
    }
  }, [page, popup]);

  useLayoutEffect(() => {
    const element = dialog.current;
    if (!point || !element) return;
    const position = () => placeDialog(element, point.x, point.y, true);
    position();
    const observer = new ResizeObserver(position);
    observer.observe(element);
    window.addEventListener("resize", position);
    return () => { observer.disconnect(); window.removeEventListener("resize", position); };
  }, [point]);

  function show(next: Page | undefined) {
    if (popup && next === undefined) returnEntry.current = page;
    if (directoryDetail && next === undefined) requestAnimationFrame(() => backButton.current?.focus({ preventScroll: true }));
    setSaveError("");
    setSaved(false);
    setPhoto(undefined);
    setPage(next);
  }

  function back() {
    if (busy || picking) return;
    if (page) show(undefined);
    else close();
  }
  useIosBack(back, 10);

  async function pick(field: ImageField) {
    const trigger = document.activeElement;
    setPicking(true);
    setSaveError("");
    try {
      const image = await api.pickImage(true);
      if (image.byteLength) {
        setPhoto(image);
        setPage(field);
        return;
      }
    } catch (error) {
      setSaveError(errorMessage(error));
    } finally {
      setPicking(false);
    }
    if (popup && trigger instanceof HTMLElement) requestAnimationFrame(() => trigger.focus());
  }

  async function save(action: () => Promise<Partial<Profile>>) {
    if (busy) return;
    setBusy(true);
    setSaveError("");
    try {
      const patch = await action();
      source.patch(user.id, patch);
      edited(user.id, patch);
      show(undefined);
      setSaved(true);
      void refresh();
    } catch (error) {
      setSaveError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const fields = new FormData(event.currentTarget);
    const value = String(fields.get("value") ?? "").trim();
    void save(async () => {
      if (page === "display_name") {
        await api.setDisplayName(value);
        return { display_name: value || null };
      }
      if (page === "bio") {
        await api.setBio(value);
        return { bio: value };
      }
      if (page === "notes") {
        const notes = { remark: value, contact_info: String(fields.get("details") ?? "").trim() };
        await api.setContactNotes(user.id, notes);
        return { notes };
      }
      await api.addContact(user.id, value);
      return { relation: "outgoing" };
    });
  }

  const workspaceImages = useContext(WorkspaceImages);
  const coverVersion = profile?.cover;
  const cachedCover = workspaceImages && shared && coverVersion;
  const { url: coverImage } = useWorkspaceImage(cachedCover ? `cover/${user.id}/${coverVersion}` : undefined,
    user.id, coverVersion ?? undefined, current?.expires_at, retry, active);
  const cover = cachedCover ? coverImage : coverVersion ? api.profileImageUrl(user.id, coverVersion) : undefined;
  const avatar = avatars[user.id] ?? profile?.avatar;
  useLayoutEffect(() => {
    if (!workspaceImages || !shared) return;
    const prefix = `cover/${user.id}/`;
    workspaceImages.cache.retain(key => !key.startsWith(prefix) || key === prefix + coverVersion);
  }, [workspaceImages?.cache, shared, user.id, coverVersion]);

  if (!active) return null;

  const name = remarks[user.id] || profile?.notes?.remark || profile?.display_name || displayName(user, remarks);
  const photoPage = page === "avatar" || page === "cover";
  const editor = page && (own || (page === "notes" || page === "remove" ? contact : profile?.relation === "stranger"));
  const title = editor ? labels[page] : own ? t("Personal information", "个人信息") : t("Profile", "个人资料");
  const saveLabel = busy ? actions.saving : saveError ? actions.retry : page === "greeting" ? t("Send", "发送") : actions.save;
  const qrName = labels.qr;
  const card = (popup || (ios && !phone && own)) && !page;
  const quickCard = popup && card;

  const content = <section className={"profile-page" + (own ? "" : " contact-profile")}
    data-contact-detail={directoryDetail ? user.id : undefined} aria-label={popup ? undefined : title} onKeyDown={event => {
    if (!popup && event.key === "Escape") { event.stopPropagation(); back(); }
  }}>
    <header className={"profile-header" + (quickCard ? " profile-card-header" : "")}>
      {!quickCard && <button className="icon-button" type="button" ref={backButton} onClick={back} disabled={busy || picking}
        aria-label={actions.back}><ChevronLeft size={25} /></button>}
      <h1 id={heading} className={quickCard ? "sr-only" : undefined}>{title}</h1>
      {contactLayout && !editor && <button className="icon-button" type="button" ref={more} disabled={busy || !contact}
        aria-label={t("Contact actions", "联系人操作")} aria-haspopup="menu" aria-expanded={!!menuAnchor && menuAnchor === more.current}
        onClick={event => openMenu(event.currentTarget, [
          { label: labels.remove, icon: <UserRoundMinus size={18} />, action: () => show("remove") },
        ])}><MoreHorizontal size={23} /></button>}
    </header>
    <div className="profile-content">
      {editor && photoPage && photo && <ProfileImageEditor key={page} source={photo} kind={page}
        disabled={busy} saveLabel={saveLabel} cancel={back} save={crop => {
          void save(async () => ({ [page]: await api.setProfileImage(page, await crop()) }));
        }} />}
      {editor && page === "qr" && <QrContent code={{ kind: "contact", name: qrName, value: session.user.handle }} server={session.server} />}
      {editor && page === "remove" && <>
        <p>{t("Remove this person from your contacts?", "删除这位联系人？")}</p>
        <div className="dialog-actions">
          <button className="button subtle" disabled={busy} onClick={back}>{actions.cancel}</button>
          <button className="button danger" disabled={busy} onClick={async () => {
            setBusy(true);
            setSaveError("");
            try {
              await api.removeContact(user.id);
              if (onContactUnavailable) onContactUnavailable(user.id);
              else close();
              void refresh();
            } catch (error) { setSaveError(errorMessage(error)); setBusy(false); }
          }}>{saveError ? actions.retry : labels.remove}</button>
        </div>
      </>}
      {editor && !photoPage && page !== "qr" && page !== "remove" && <form className="profile-fields" key={page} onSubmit={submit}>
        {page === "bio" || page === "greeting"
            ? <FloatingField multiline label={labels[page]} name="value" maxLength={page === "bio" ? 300 : 200} rows={4} readOnly={busy} autoFocus
              defaultValue={page === "bio" ? profile?.bio ?? "" : ""} />
            : <FloatingField label={page === "notes" ? t("Remark", "备注名") : labels[page]} name="value" maxLength={64} readOnly={busy} autoFocus
              defaultValue={page === "notes" ? profile?.notes?.remark ?? "" : profile?.display_name ?? ""} />}
        {page === "notes" && <>
          <FloatingField multiline label={t("Contact details", "联系资料")} name="details" rows={3} maxLength={300} readOnly={busy} defaultValue={profile?.notes?.contact_info ?? ""} />
          <small>{t("Only you can see these details.", "仅自己可见。")}</small>
        </>}
        <div className="dialog-actions">
          <button type="button" className="button subtle" onClick={back} disabled={busy}>{actions.cancel}</button>
          <button className="button primary" disabled={busy}>{saveLabel}</button>
        </div>
      </form>}
      {!editor && <>
        {card ? <>
          {coverVersion && <div className="profile-cover">{cover && <img src={cover} alt="" />}</div>}
          <PersonAvatar user={user} version={avatar} large />
          <dl className="quick-profile-fields">
            {[
              [t("Nickname", "昵称"), profile?.display_name],
              [t("Username", "账号"), user.handle],
              [t("Remark", "备注"), contact && relationVersion === "contact" ? profile?.notes?.remark : ""],
              [t("Phone", "电话"), ""],
              [t("Signature", "个性签名"), profile?.bio],
              [t("Added via", "添加方式"), ""],
              [t("Date added", "添加日期"), ""],
            ].map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}
          </dl>
          {contactLayout && <button className="button subtle" disabled={busy || !contact} onClick={async () => {
            setBusy(true);
            try { await message(); } catch (error) { setSaveError(errorMessage(error)); setBusy(false); }
          }}>{t("Message", "发消息")}</button>}
          {profile?.relation === "stranger" && <button className="button primary" onClick={() => show("greeting")}>{labels.greeting}</button>}
          {profile?.relation === "outgoing" && <p role="status">{t("Contact request sent.", "好友请求已发送。")}</p>}
        </> : own ? <div className="profile-rows">
          <Row label={labels.avatar} entry="avatar" choose={() => void pick("avatar")} disabled={!profile || picking}>
            <PersonAvatar user={user} version={avatar} large />
          </Row>
          <Row label={labels.display_name} entry="display_name" choose={() => show("display_name")} disabled={!profile}>{name}</Row>
          <div className="profile-row profile-static"><span>{t("Username", "账号")}</span><span className="profile-value">@{user.handle}</span></div>
          <Row label={labels.cover} entry="cover" choose={() => void pick("cover")} disabled={!profile || picking}>
            {cover && <img className="profile-cover-thumb" src={cover} alt="" />}
          </Row>
          <Row label={labels.bio} entry="bio" choose={() => show("bio")} disabled={!profile}>{profile?.bio}</Row>
          {phone && <Row label={qrName} entry="qr" choose={() => show("qr")}><QrCode size={21} /></Row>}
        </div> : <>
          <div className="profile-person"><PersonAvatar user={user} version={avatar} large />
            <div><h2>{name}</h2><small className="muted">@{user.handle}</small></div>
          </div>
          {coverVersion && <div className="profile-cover">{cover && <img src={cover} alt="" />}</div>}
          {profile?.bio && <p className="profile-bio">{profile.bio}</p>}
          {contactLayout && <div className="profile-rows"><Row label={labels.notes} disabled={!contact} choose={() => show("notes")}>{profile?.notes?.remark}</Row></div>}
          {contactLayout && <button className="button primary profile-direct" disabled={busy || !contact} onClick={async () => {
            setBusy(true);
            setSaveError("");
            try { await message(); }
            catch (error) { setSaveError(errorMessage(error)); }
            finally { setBusy(false); }
          }}><MessageCircle size={18} /> {t("Message", "发消息")}</button>}
          {profile?.relation === "stranger" && <button className="button primary profile-direct" onClick={() => show("greeting")}>
            {t("Add contact", "添加联系人")}
          </button>}
          {profile?.relation === "incoming" && <p className="muted">{t("Respond to this request in Contacts.", "在联系人中处理此请求。")}</p>}
          {profile?.relation === "outgoing" && <p className="muted" role="status">{t("Contact request sent.", "好友请求已发送。")}</p>}
        </>}
        <div className="profile-status" role="status">
          {loadError ? <><span>{loadError}</span><button className="text-button" onClick={() => setRetry(value => value + 1)}>{actions.retry}</button></>
            : saved ? actions.saved : !profile ? t("Loading profile…", "正在读取资料…") : null}
        </div>
      </>}
      {saveError && <p className="inline-error" role="alert">{saveError}</p>}
    </div>
  </section>;
  if (!popup) return content;
  return <dialog className="profile-popup" ref={dialog} aria-labelledby={heading}
    onCancel={event => { event.preventDefault(); back(); }}
    onPointerDown={event => {
      const { left, top, right, bottom } = event.currentTarget.getBoundingClientRect();
      outsidePress.current = event.target === event.currentTarget
        && (event.clientX < left || event.clientX >= right || event.clientY < top || event.clientY >= bottom);
    }}
    onClick={event => {
      if (outsidePress.current && event.target === event.currentTarget) back();
      outsidePress.current = false;
    }}>
    {content}
  </dialog>;
}
