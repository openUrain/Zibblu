import { Camera, ImagePlus, MessageSquare, QrCode, UserPlus, UserRound } from "lucide-react";
import { useContext, useEffect, useState } from "react";
import { api, errorMessage } from "./api";
import type { Profile, User } from "./api";
import { MAX_IMAGE_BYTES } from "./attachment-media";
import { Blank, Dialog, People, PersonAvatar, displayName } from "./common";

const imageTypes = "image/png,image/apng,image/jpeg,image/webp,image/gif,image/bmp,.apng";
const imageHelp = "PNG, JPEG, WebP, GIF or BMP up to 16 MiB";

export function ProfileView({ user, relationVersion, close, refresh, saved, message, code }: {
  user: User; relationVersion: string; close: () => void; refresh: () => Promise<void>;
  saved: () => void; message: () => Promise<void>; code: () => void;
}) {
  const { remarks } = useContext(People);
  const [profile, setProfile] = useState<Profile>();
  const [retry, setRetry] = useState(0);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [asking, setAsking] = useState(false);
  const [pendingImage, setPendingImage] = useState<{ kind: "avatar" | "cover"; file: File }>();

  useEffect(() => {
    let current = true;
    setProfile(undefined);
    setFailure("");
    void api.profile(user.id).then(value => { if (current) setProfile(value); })
      .catch(error => { if (current) setFailure(errorMessage(error)); });
    return () => { current = false; };
  }, [user.id, relationVersion, retry]);

  async function save(action: () => Promise<void>, closeOnSuccess = false) {
    if (busy) return;
    setBusy(true);
    setFailure("");
    try {
      await action();
      if (closeOnSuccess) saved();
      else setProfile(await api.profile(user.id));
      await refresh();
    }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  function choose(kind: "avatar" | "cover", file?: File) {
    if (!file) return;
    if (file.size > MAX_IMAGE_BYTES) {
      setPendingImage(undefined);
      setFailure(`Choose a ${imageHelp}.`);
      return;
    }
    setFailure("");
    setPendingImage({ kind, file });
  }

  const name = profile?.notes?.remark || displayName(user, remarks);
  const own = profile?.relation === "own";
  const contact = profile?.relation === "contact";
  return <Dialog title={own ? "Your profile" : "Profile"} className="profile-dialog" close={close} closeDisabled={busy}>
    {!profile && (failure ? <Blank icon={<UserRound size={24} />} title="Profile unavailable">
      <p role="alert">{failure}</p><button className="button subtle" onClick={() => setRetry(value => value + 1)}>Retry</button>
    </Blank> : <p className="loading muted" role="status">Loading profile…</p>)}
    {profile && <>
      <div className={"profile-banner" + (!own && !contact ? " profile-banner-basic" : "")}>
        {profile.cover && <img src={api.profileImageUrl(user.id, profile.cover)} alt="" />}
        {own && <label className="button profile-image-control" title={imageHelp}><ImagePlus size={16} /> Change cover
          <input type="file" accept={imageTypes} disabled={busy} onChange={event => {
            choose("cover", event.currentTarget.files?.[0]); event.currentTarget.value = "";
          }} />
        </label>}
      </div>
      <div className="profile-identity">
        <div className="profile-avatar"><PersonAvatar user={profile.user} version={profile.avatar} large />
          {own && <label className="icon-button profile-image-control" title={`Change avatar · ${imageHelp}`}><Camera size={16} />
            <input type="file" aria-label="Change avatar" accept={imageTypes} disabled={busy} onChange={event => {
              choose("avatar", event.currentTarget.files?.[0]); event.currentTarget.value = "";
            }} />
          </label>}
        </div>
        <div><h3>{name}</h3>{name !== profile.user.handle && <p className="muted">@{profile.user.handle}</p>}</div>
        {own && <button className="button subtle" disabled={busy} onClick={code}><QrCode size={17} /> Your QR code</button>}
        {contact && <button className="button subtle" disabled={busy} onClick={async () => {
          setBusy(true);
          setFailure("");
          try { await message(); }
          catch (error) { setFailure(errorMessage(error)); setBusy(false); }
        }}><MessageSquare size={17} /> Message</button>}
        {profile.relation === "stranger" && !asking && <button className="button primary" onClick={() => setAsking(true)}><UserPlus size={17} /> Add contact</button>}
      </div>
      {own && pendingImage && <div className="profile-image-approval">
        <p role="status"><strong>{pendingImage.file.name}</strong></p>
        <p className="muted">The image is resized to fit. Animations use the first frame.</p>
        <div className="dialog-actions">
          <button className="button subtle" disabled={busy} onClick={() => { setPendingImage(undefined); setFailure(""); }}>Cancel</button>
          <button className="button primary" disabled={busy} onClick={() => void save(async () => {
            await api.setProfileImage(pendingImage.kind, await pendingImage.file.arrayBuffer());
            setPendingImage(undefined);
          })}>{busy ? "Saving…" : pendingImage.kind === "avatar" ? "Use as avatar" : "Use as cover"}</button>
        </div>
      </div>}
      {own && <form className="profile-fields" key={profile.bio} onSubmit={event => {
        event.preventDefault();
        const bio = String(new FormData(event.currentTarget).get("bio") ?? "");
        void save(() => api.setBio(bio), true);
      }}>
        <label>Bio<textarea name="bio" rows={3} maxLength={300} defaultValue={profile.bio ?? ""} readOnly={busy} /></label>
        <button className="button primary" disabled={busy}>Save bio</button>
      </form>}
      {contact && <>
        {profile.bio && <p className="profile-bio">{profile.bio}</p>}
        {profile.notes && <form className="profile-fields" key={JSON.stringify(profile.notes)} onSubmit={event => {
          event.preventDefault();
          const fields = new FormData(event.currentTarget);
          void save(() => api.setContactNotes(user.id, {
            remark: String(fields.get("remark") ?? ""), contact_info: String(fields.get("contact_info") ?? ""),
          }), true);
        }}>
          <label>Remark<input name="remark" defaultValue={profile.notes.remark} maxLength={64} readOnly={busy} /></label>
          <label>Contact details <small>Only you can see these details</small>
            <textarea name="contact_info" defaultValue={profile.notes.contact_info} maxLength={300} rows={2} readOnly={busy} />
          </label>
          <button className="button primary" disabled={busy}>Save contact details</button>
        </form>}
      </>}
      {asking && profile.relation === "stranger" && <form className="profile-fields" onSubmit={event => {
        event.preventDefault();
        const greeting = String(new FormData(event.currentTarget).get("greeting") ?? "");
        void save(() => api.addContact(user.id, greeting));
      }}>
        <label>Greeting <small>Sent as a message after approval</small><textarea name="greeting" rows={3} maxLength={200} autoFocus /></label>
        <button className="button primary" disabled={busy}>Send contact request</button>
      </form>}
      {profile.relation === "incoming" && <p className="muted">Respond to this request in Contacts.</p>}
      {profile.relation === "outgoing" && <p className="muted" role="status">Contact request sent.</p>}
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </>}
  </Dialog>;
}
