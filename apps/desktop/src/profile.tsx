import { FloatingField } from "./floating-field";
import { ChevronLeft, ChevronRight, MessageCircle, MoreHorizontal, QrCode, UserRoundMinus } from "lucide-react";
import { useContext, useEffect, useRef, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { api, errorMessage } from "./api";
import type { CachedProfile, Profile, Session, User } from "./api";
import { People, PersonAvatar, displayName } from "./common";
import { actions, t } from "./locale";
import { useIosBack } from "./ios";
import { QrContent } from "./qr";
import { ProfileImageEditor } from "./profile-image";
import { useMenu, useMenuAnchor } from "./context-menu";

type ImageField = "avatar" | "cover";
type Page = ImageField | "display_name" | "bio" | "notes" | "greeting" | "qr" | "remove";

const labels: Record<Page, string> = {
  avatar: t("Avatar", "头像"), cover: t("Cover", "封面"), display_name: t("Name", "名称"),
  bio: t("Bio", "简介"), notes: t("Contact notes", "备注与资料"), greeting: t("Contact request", "添加联系人"),
  qr: t("My QR code", "我的二维码"),
  remove: t("Remove contact", "删除联系人"),
};

function Row({ label, children, choose, disabled }: {
  label: string; children?: ReactNode; choose: () => void; disabled?: boolean;
}) {
  return <button className="profile-row" disabled={disabled} onClick={choose}>
    <span>{label}</span><span className="profile-value">{children}</span><ChevronRight size={17} aria-hidden />
  </button>;
}

export function ProfileView({ session, user, relationVersion, reconnecting, close, refresh, edited, message }: {
  session: Session; user: User; relationVersion: string; reconnecting: boolean;
  close: () => void; refresh: () => Promise<void>; edited: (id: string, patch: Partial<Profile>) => void;
  message: () => Promise<void>;
}) {
  const { remarks } = useContext(People);
  const openMenu = useMenu();
  const menuAnchor = useMenuAnchor();
  const more = useRef<HTMLButtonElement>(null);
  const own = user.id === session.user.id;
  const [snapshot, setSnapshot] = useState<{ data: CachedProfile; relation: string }>();
  const [page, setPage] = useState<Page>();
  const [photo, setPhoto] = useState<ArrayBuffer>();
  const [picking, setPicking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saveError, setSaveError] = useState("");
  const [loadError, setLoadError] = useState("");
  const [saved, setSaved] = useState(false);
  const [retry, setRetry] = useState(0);
  const serial = useRef(0);
  const backButton = useRef<HTMLButtonElement>(null);
  const current = snapshot?.relation === relationVersion ? snapshot.data : undefined;
  const profile = current?.profile;
  const contact = profile?.relation === "contact";

  useEffect(() => {
    const load = ++serial.current;
    async function read() {
      setLoadError("");
      const cached = await api.cachedProfile(user.id).catch(() => null);
      if (serial.current !== load) return;
      if (cached) setSnapshot(previous => previous?.relation === relationVersion ? previous : { data: cached, relation: relationVersion });
      try {
        const data = await api.profile(user.id);
        if (serial.current === load) setSnapshot({ data, relation: relationVersion });
      } catch (error) {
        if (serial.current !== load) return;
        const status = typeof error === "object" && error && "status" in error ? error.status : undefined;
        if (status === 401 || status === 403 || status === 404) setSnapshot(undefined);
        setLoadError(errorMessage(error));
      }
    }
    void read();
    return () => { ++serial.current; };
  }, [user.id, relationVersion, retry, reconnecting]);

  useEffect(() => {
    if (!current) return;
    const timer = window.setTimeout(() => {
      setSnapshot(undefined);
      setRetry(value => value + 1);
    }, Math.max(0, current.expires_at - Date.now()));
    return () => window.clearTimeout(timer);
  }, [current]);

  useEffect(() => {
    if (page === undefined) backButton.current?.focus({ preventScroll: true });
  }, [page]);

  function show(next: Page | undefined) {
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
    setPicking(true);
    setSaveError("");
    try {
      const image = await api.pickImage(true);
      if (image.byteLength) {
        setPhoto(image);
        setPage(field);
      }
    } catch (error) {
      setSaveError(errorMessage(error));
    } finally {
      setPicking(false);
    }
  }

  async function save(action: () => Promise<Partial<Profile>>) {
    if (busy) return;
    setBusy(true);
    setSaveError("");
    try {
      const patch = await action();
      ++serial.current;
      setSnapshot(previous => previous && { ...previous, data: {
        ...previous.data, profile: { ...previous.data.profile, ...patch },
      } });
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

  const name = profile?.notes?.remark || profile?.display_name || displayName(user, remarks);
  const photoPage = page === "avatar" || page === "cover";
  const editor = page && (own || (page === "notes" || page === "remove" ? contact : profile?.relation === "stranger"));
  const title = editor ? labels[page] : own ? t("Personal information", "个人信息") : t("Profile", "个人资料");
  const saveLabel = busy ? actions.saving : saveError ? actions.retry : page === "greeting" ? t("Send", "发送") : actions.save;
  const qrName = labels.qr;

  return <section className={"profile-page" + (own ? "" : " contact-profile")} aria-label={title} onKeyDown={event => {
    if (event.key === "Escape") { event.stopPropagation(); back(); }
  }}>
    <header className="profile-header">
      <button className="icon-button" type="button" ref={backButton} onClick={back} disabled={busy || picking} aria-label={actions.back}>
        <ChevronLeft size={25} />
      </button>
      <h1>{title}</h1>
      {contact && !editor && <button className="icon-button" type="button" ref={more} disabled={busy}
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
              close();
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
        {own ? <div className="profile-rows">
          <Row label={labels.avatar} choose={() => void pick("avatar")} disabled={!profile || picking}>
            <PersonAvatar user={user} version={profile?.avatar} large />
          </Row>
          <Row label={labels.display_name} choose={() => show("display_name")} disabled={!profile}>{name}</Row>
          <div className="profile-row profile-static"><span>{t("Username", "账号")}</span><span className="profile-value">@{user.handle}</span></div>
          <Row label={labels.cover} choose={() => void pick("cover")} disabled={!profile || picking}>
            {profile?.cover && <img className="profile-cover-thumb" src={api.profileImageUrl(user.id, profile.cover)} alt="" />}
          </Row>
          <Row label={labels.bio} choose={() => show("bio")} disabled={!profile}>{profile?.bio}</Row>
          <Row label={qrName} choose={() => show("qr")}><QrCode size={21} /></Row>
        </div> : <>
          <div className="profile-person"><PersonAvatar user={user} version={profile?.avatar} large />
            <div><h2>{name}</h2><small className="muted">@{user.handle}</small></div>
          </div>
          {profile?.cover && <img className="profile-cover" src={api.profileImageUrl(user.id, profile.cover)} alt="" />}
          {profile?.bio && <p className="profile-bio">{profile.bio}</p>}
          {contact && <div className="profile-rows"><Row label={labels.notes} choose={() => show("notes")}>{profile.notes?.remark}</Row></div>}
          {contact && <button className="button primary profile-direct" disabled={busy} onClick={async () => {
            setBusy(true);
            setSaveError("");
            try { await message(); }
            catch (error) { setSaveError(errorMessage(error)); setBusy(false); }
          }}><MessageCircle size={18} /> {t("Message", "发消息")}</button>}
          {profile?.relation === "stranger" && <button className="button primary profile-direct" onClick={() => show("greeting")}>
            {t("Add contact", "添加联系人")}
          </button>}
          {profile?.relation === "incoming" && <p className="muted">{t("Respond to this request in Contacts.", "在联系人中处理此请求。")}</p>}
          {profile?.relation === "outgoing" && <p className="muted" role="status">{t("Contact request sent.", "好友请求已发送。")}</p>}
        </>}
        {!profile && !loadError && <p role="status" className="muted">{t("Loading profile…", "正在读取资料…")}</p>}
        {loadError && <div className="profile-status" role="status">
          <span>{loadError}</span><button className="text-button" onClick={() => setRetry(value => value + 1)}>{actions.retry}</button>
        </div>}
        {saved && <p role="status" className="profile-status">{actions.saved}</p>}
      </>}
      {saveError && <p className="inline-error" role="alert">{saveError}</p>}
    </div>
  </section>;
}
