import { FloatingField } from "./floating-field";
import { ArrowRight, Eye, EyeOff, Globe2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, errorMessage, isExpired } from "./api";
import type { CloseAction, Preferences, SavedAccount, Session } from "./api";
import { Avatar, Brand } from "./common";
import { DesktopMenus } from "./context-menu";
import { setupIosLayout } from "./ios";
import { Workspace } from "./workspace";
import { ChatWindow } from "./chat-window";
import { PrivateInvitationWindow } from "./private-chat";
import { ImageViewer } from "./image-viewer";
import { WindowControls, WindowClosePrompt, WindowDrag, windows } from "./window-chrome";
import "./style.css";
import "./ios.css";

function App() {
  const [preferences, setPreferences] = useState<Preferences>();
  const [settingsError, setSettingsError] = useState("");
  const [session, setSession] = useState<Session>();
  const [notice, setNotice] = useState("");

  useEffect(() => {
    api.preferences().then(setPreferences).catch(error => setSettingsError(errorMessage(error)));
  }, []);
  async function setCloseAction(action: CloseAction) {
    await api.setCloseAction(action);
    setPreferences(current => current && { ...current, close_action: action });
  }
  const signedOut = useCallback((message?: string) => {
    setNotice(message ?? "");
    setSession(undefined);
  }, []);

  return <>
    {windows && !session && <header className="auth-titlebar">
      <WindowDrag /><WindowControls />
    </header>}
    {settingsError ? <main className="boot"><Brand /><p className="inline-error" role="alert">{settingsError}</p></main>
    : !preferences ? <main className="boot"><Brand /><span role="status">Opening Zibblu…</span></main>
    : session ? <Workspace session={session} closeAction={preferences.close_action} setCloseAction={setCloseAction}
      privateQuiet={preferences.private_quiet} setPrivateQuiet={async quiet => {
        await api.setPrivateQuiet(quiet);
        setPreferences(current => current && { ...current, private_quiet: quiet });
      }} signedOut={signedOut} />
    : <SignIn initialServer={preferences.server} officialServer={preferences.official_server}
      initialAccount={preferences.saved_account} notice={notice} signedIn={result => {
      setPreferences(current => current && { ...current, server: result.server, saved_account: undefined });
      setSession(result);
    }} />}
    {windows && <WindowClosePrompt save={setCloseAction} />}
  </>;
}

function SignIn({ initialServer, officialServer, initialAccount, notice, signedIn }: {
  initialServer: string; officialServer: string; initialAccount?: SavedAccount; notice: string; signedIn: (session: Session) => void;
}) {
  const [saved, setSaved] = useState(initialAccount);
  const [useSaved, setUseSaved] = useState(true);
  const [avatarUrl, setAvatarUrl] = useState<string>();
  const [register, setRegister] = useState(false);
  const [server, setServer] = useState(initialServer);
  const [advanced, setAdvanced] = useState(false);
  const [handle, setHandle] = useState("");
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const savedButton = useRef<HTMLButtonElement>(null);
  const usernameInput = useRef<HTMLInputElement>(null);
  const passwordInput = useRef<HTMLInputElement>(null);
  const confirmationInput = useRef<HTMLInputElement>(null);
  const nextFocus = useRef<"username" | "password">(undefined);
  const [passwordVisible, setPasswordVisible] = useState(false);
  const [checkConfirmation, setCheckConfirmation] = useState(false);
  const [failure, setFailure] = useState(notice);
  const [busy, setBusy] = useState(false);
  const account = useSaved ? saved : undefined;
  const official = server.trim().replace(/\/$/, "") === officialServer;
  const mismatch = register && confirmation !== password;
  const showMismatch = mismatch && checkConfirmation;

  useEffect(() => {
    if (!account?.avatar) return;
    const url = URL.createObjectURL(new Blob([new Uint8Array(account.avatar)], { type: "image/png" }));
    setAvatarUrl(url);
    return () => URL.revokeObjectURL(url);
  }, [account]);

  useEffect(() => {
    if (busy) return;
    if (account) savedButton.current?.focus();
    else if (nextFocus.current) {
      (nextFocus.current === "password" ? passwordInput : usernameInput).current?.focus();
      nextFocus.current = undefined;
    }
  }, [account, busy]);

  function chooseServer(address: string) {
    setServer(address);
    resetPassword();
  }

  function resetPassword() {
    setPassword("");
    setConfirmation("");
    setPasswordVisible(false);
    setCheckConfirmation(false);
    setFailure("");
  }

  async function restore() {
    if (busy || !account) return;
    setBusy(true);
    setFailure("");
    try { signedIn(await api.restoreSession(account)); }
    catch (error) {
      if (isExpired(error)) {
        setSaved(undefined);
        setHandle(account.user.handle);
        nextFocus.current = "password";
      }
      setFailure(errorMessage(error));
    } finally { setBusy(false); }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    if (mismatch) {
      setCheckConfirmation(true);
      confirmationInput.current?.focus();
      return;
    }
    setBusy(true);
    setFailure("");
    try {
      signedIn(await api.login(server, handle, password, register));
    } catch (error) {
      setFailure(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  return <main className="signin">
    <section className="signin-brand">
      <div className="wordmark"><Brand /><span>Zibblu</span></div>
      <div className="signin-message">
        <h1>Keep the conversation going.</h1>
        <p>Messages, group chats, and files in one place.</p>
      </div>
      <div className="signin-connection"><Globe2 size={17} /> Normal online chat</div>
    </section>
    <section className="signin-form">
      {account ? <div className="signin-saved">
        <h2>Continue to Zibblu</h2>
        <button className="button signin-account" type="button" ref={savedButton} disabled={busy}
          aria-label={`Continue as ${account.user.handle}`} aria-describedby="saved-account-server"
          onClick={() => void restore()}>
          <Avatar name={account.user.handle} large src={avatarUrl} />
          <span><strong>{account.user.handle}</strong>
            <small id="saved-account-server">{official ? "Zibblu Online" : account.server}</small>
          </span>
          <ArrowRight size={19} aria-hidden />
        </button>
        {busy && <p className="muted" role="status">Connecting…</p>}
        {failure && <p className="inline-error" role="alert">{failure}</p>}
        <button className="text-button" disabled={busy} onClick={() => {
          nextFocus.current = "username";
          setUseSaved(false);
          setHandle("");
          resetPassword();
        }}>Use another account</button>
      </div> : <div>
        <h2>{register ? "Create an account" : "Sign in to Zibblu"}</h2>
        <p className="muted">{register ? "Choose a username friends can use to find you." : "Use your Zibblu username and password."}</p>
        <form onSubmit={submit}>
          <div className="signin-server">
            <Globe2 size={18} aria-hidden="true" />
            <div>
              <strong>{official ? "Zibblu Online" : "Custom server"}</strong>
              <span className="muted">{official ? "Official server" : server.trim() || "Enter a server address"}</span>
            </div>
            <button className="text-button" type="button" disabled={busy} aria-expanded={advanced}
              aria-controls="signin-advanced" onClick={() => setAdvanced(!advanced)}>Advanced</button>
          </div>
          <div id="signin-advanced" hidden={!advanced}>
            {advanced && <>
              <FloatingField label="Server address" id="server" disabled={busy} type="url" value={server} onChange={event => chooseServer(event.target.value)} required />
              {!official && <div className="signin-server-help">
                <p className="muted">Accounts and chat history belong to the selected server.</p>
                <button className="text-button" type="button" disabled={busy} onClick={() => {
                  chooseServer(officialServer);
                  setAdvanced(false);
                }}>Use Zibblu Online</button>
              </div>}
            </>}
          </div>
          <FloatingField label="Username" id="username" ref={usernameInput} value={handle} onChange={event => setHandle(event.target.value)} required
              autoComplete="username" maxLength={32} />
          <FloatingField label="Password" id="password" fieldClass="password-field" trailingAction={
              <button className="icon-button" type="button" aria-label={`${passwordVisible ? "Hide" : "Show"} ${register ? "passwords" : "password"}`}
                aria-controls={register ? "password confirm-password" : "password"} onClick={() => setPasswordVisible(!passwordVisible)}>
                {passwordVisible ? <EyeOff size={18} /> : <Eye size={18} />}
              </button>
            } ref={passwordInput} data-password type={passwordVisible ? "text" : "password"} value={password}
                onChange={event => setPassword(event.target.value)} required
                autoComplete={register ? "new-password" : "current-password"} minLength={register ? 12 : undefined}
                maxLength={256} />
          {register && <FloatingField label="Confirm password" id="confirm-password" ref={confirmationInput} data-password type={passwordVisible ? "text" : "password"} value={confirmation}
              onChange={event => setConfirmation(event.target.value)}
              required autoComplete="new-password" maxLength={256} aria-invalid={showMismatch || undefined}
              aria-describedby={showMismatch ? "password-mismatch" : undefined} />}
          {showMismatch && <p id="password-mismatch" className="inline-error" role="alert">Passwords don’t match.</p>}
          {failure && <div className="inline-error" role="alert">{failure}</div>}
          <button className="button primary signin-submit" disabled={busy} type="submit">
            {busy ? "Connecting…" : register ? "Create account" : "Sign in"} <ArrowRight size={17} />
          </button>
        </form>
        <div className="signin-switch">{register ? "Already have an account?" : "New to Zibblu?"}{" "}
          <button className="text-button" onClick={() => { setRegister(!register); resetPassword(); }} disabled={busy}>
            {register ? "Sign in" : "Create an account"}
          </button>
        </div>
        {saved && <div className="signin-switch"><button className="text-button" disabled={busy} onClick={() => {
          chooseServer(saved.server);
          setRegister(false);
          setAdvanced(false);
          setUseSaved(true);
        }}>Back to saved account</button></div>}
      </div>}
    </section>
  </main>;
}

document.body.classList.toggle("windows", windows);
setupIosLayout();
const label = getCurrentWindow().label;
const isChatWindow = /^chat-[0-9a-f]{32}$/i.test(label);
createRoot(document.getElementById("root")!).render(label === "image-viewer" ? <ImageViewer /> : <DesktopMenus>
  {label === "private-invitation" ? <PrivateInvitationWindow /> : isChatWindow ? <ChatWindow /> : <App />}
</DesktopMenus>);
