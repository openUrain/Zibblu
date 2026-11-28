import { FloatingField } from "./floating-field";
import { ArrowRight, Eye, EyeOff, Globe2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
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
import { appLanguage, applyAppLanguage, LanguageContext, t } from "./locale";
import type { AppLanguage } from "./locale";
import "./style.css";
import "./ios.css";

function App({ initialPreferences, settingsError }: { initialPreferences?: Preferences; settingsError: string }) {
  const [preferences, setPreferences] = useState(initialPreferences);
  const [session, setSession] = useState<Session>();
  const [notice, setNotice] = useState("");

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
    : !preferences ? <main className="boot"><Brand /><span role="status">{t("Opening Zibblu…", "正在打开 Zibblu…")}</span></main>
    : session ? <Workspace session={session} closeAction={preferences.close_action} setCloseAction={setCloseAction}
      language={appLanguage()} setLanguage={async language => {
        await api.setAppLanguage(language);
        renderApp(language);
      }}
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
        <h1>{t("Keep the conversation going.", "让聊天继续。")}</h1>
        <p>{t("Messages, group chats, and files in one place.", "随时聊天、建群和分享文件。")}</p>
      </div>
      <div className="signin-connection"><Globe2 size={17} />{t("Normal online chat", "普通在线聊天")}</div>
    </section>
    <section className="signin-form">
      {account ? <div className="signin-saved">
        <h2>{t("Continue to Zibblu", "继续使用 Zibblu")}</h2>
        <button className="button signin-account" type="button" ref={savedButton} disabled={busy}
          aria-label={t(`Continue as ${account.user.handle}`, `以 ${account.user.handle} 的身份继续`)} aria-describedby="saved-account-server"
          onClick={() => void restore()}>
          <Avatar name={account.user.handle} large src={avatarUrl} />
          <span><strong>{account.user.handle}</strong>
            <small id="saved-account-server">{official ? t("Zibblu Online", "Zibblu 在线服务") : account.server}</small>
          </span>
          <ArrowRight size={19} aria-hidden />
        </button>
        {busy && <p className="muted" role="status">{t("Connecting…", "正在连接…")}</p>}
        {failure && <p className="inline-error" role="alert">{failure}</p>}
        <button className="text-button" disabled={busy} onClick={() => {
          nextFocus.current = "username";
          setUseSaved(false);
          setHandle("");
          resetPassword();
        }}>{t("Use another account", "使用其他账号")}</button>
      </div> : <div>
        <h2>{register ? t("Create an account", "创建账号") : t("Sign in to Zibblu", "登录 Zibblu")}</h2>
        <p className="muted">{register ? t("Choose a username friends can use to find you.", "设置一个用户名，方便好友找到你。") : t("Use your Zibblu username and password.", "使用 Zibblu 用户名和密码登录。")}</p>
        <form onSubmit={submit}>
          <div className="signin-server">
            <Globe2 size={18} aria-hidden="true" />
            <div>
              <strong>{official ? t("Zibblu Online", "Zibblu 在线服务") : t("Custom server", "自定义服务器")}</strong>
              <span className="muted">{official ? t("Official server", "官方服务器") : server.trim() || t("Enter a server address", "输入服务器地址")}</span>
            </div>
            <button className="text-button" type="button" disabled={busy} aria-expanded={advanced}
              aria-controls="signin-advanced" onClick={() => setAdvanced(!advanced)}>{t("Advanced", "高级")}</button>
          </div>
          <div id="signin-advanced" hidden={!advanced}>
            {advanced && <>
              <FloatingField label={t("Server address", "服务器地址")} id="server" disabled={busy} type="url" value={server} onChange={event => chooseServer(event.target.value)} required />
              {!official && <div className="signin-server-help">
                <p className="muted">{t("Accounts and chat history belong to the selected server.", "账号和聊天记录与所选服务器关联。")}</p>
                <button className="text-button" type="button" disabled={busy} onClick={() => {
                  chooseServer(officialServer);
                  setAdvanced(false);
                }}>{t("Use Zibblu Online", "使用 Zibblu 在线服务")}</button>
              </div>}
            </>}
          </div>
          <FloatingField label={t("Username", "用户名")} id="username" ref={usernameInput} value={handle} onChange={event => setHandle(event.target.value)} required
              autoComplete="username" maxLength={32} />
          <FloatingField label={t("Password", "密码")} id="password" fieldClass="password-field" trailingAction={
              <button className="icon-button" type="button" aria-label={passwordVisible ? t("Hide password", "隐藏密码") : t("Show password", "显示密码")}
                aria-controls={register ? "password confirm-password" : "password"} onClick={() => setPasswordVisible(!passwordVisible)}>
                {passwordVisible ? <EyeOff size={18} /> : <Eye size={18} />}
              </button>
            } ref={passwordInput} data-password type={passwordVisible ? "text" : "password"} value={password}
                onChange={event => setPassword(event.target.value)} required
                autoComplete={register ? "new-password" : "current-password"} minLength={register ? 12 : undefined}
                maxLength={256} />
          {register && <FloatingField label={t("Confirm password", "确认密码")} id="confirm-password" ref={confirmationInput} data-password type={passwordVisible ? "text" : "password"} value={confirmation}
              onChange={event => setConfirmation(event.target.value)}
              required autoComplete="new-password" maxLength={256} aria-invalid={showMismatch || undefined}
              aria-describedby={showMismatch ? "password-mismatch" : undefined} />}
          {showMismatch && <p id="password-mismatch" className="inline-error" role="alert">{t("Passwords don’t match.", "两次输入的密码不一致。")}</p>}
          {failure && <div className="inline-error" role="alert">{failure}</div>}
          <button className="button primary signin-submit" disabled={busy} type="submit">
            {busy ? t("Connecting…", "正在连接…") : register ? t("Create account", "创建账号") : t("Sign in", "登录")} <ArrowRight size={17} />
          </button>
        </form>
        <div className="signin-switch">{register ? t("Already have an account?", "已有账号？") : t("New to Zibblu?", "首次使用 Zibblu？")}{" "}
          <button className="text-button" onClick={() => { setRegister(!register); resetPassword(); }} disabled={busy}>
            {register ? t("Sign in", "登录") : t("Create an account", "创建账号")}
          </button>
        </div>
        {saved && <div className="signin-switch"><button className="text-button" disabled={busy} onClick={() => {
          chooseServer(saved.server);
          setRegister(false);
          setAdvanced(false);
          setUseSaved(true);
        }}>{t("Back to saved account", "返回已保存的账号")}</button></div>}
      </div>}
    </section>
  </main>;
}

document.body.classList.toggle("windows", windows);
setupIosLayout();
const label = getCurrentWindow().label;
const isChatWindow = /^chat-[0-9a-f]{32}$/i.test(label);
const root = createRoot(document.getElementById("root")!);
let initialPreferences: Preferences | undefined;
let settingsError = "";
let ready = false;

function renderApp(language?: AppLanguage) {
  applyAppLanguage(language);
  root.render(<LanguageContext.Provider value={appLanguage()}>
    {label === "image-viewer" ? <ImageViewer /> : <DesktopMenus>
      {label === "private-invitation" ? <PrivateInvitationWindow /> : isChatWindow ? <ChatWindow />
        : <App initialPreferences={initialPreferences} settingsError={settingsError} />}
    </DesktopMenus>}
  </LanguageContext.Provider>);
}

void (async () => {
  let activeLanguage: AppLanguage | undefined;
  await listen<AppLanguage>("zibblu-app-language", event => {
    activeLanguage = event.payload;
    if (ready) renderApp(activeLanguage);
  }).catch(() => {});
  let savedLanguage: AppLanguage | null | undefined;
  try {
    if (label === "main") {
      initialPreferences = await api.preferences();
      savedLanguage = initialPreferences.app_language;
    } else {
      savedLanguage = await api.appLanguage();
    }
  }
  catch (error) { settingsError = errorMessage(error); }
  ready = true;
  renderApp(activeLanguage ?? savedLanguage ?? undefined);
})();
