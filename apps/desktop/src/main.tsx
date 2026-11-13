import { ArrowRight, Globe2 } from "lucide-react";
import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, errorMessage } from "./api";
import type { Preferences, Session, Theme } from "./api";
import { Brand } from "./common";
import { DesktopMenus } from "./context-menu";
import { Workspace } from "./workspace";
import { PrivateInvitationWindow } from "./private-chat";
import "./style.css";

const defaults: Preferences = { server: "http://127.0.0.1:7841", theme: "system", private_quiet: false };

function App() {
  const [preferences, setPreferences] = useState<Preferences>();
  const [session, setSession] = useState<Session>();
  const [notice, setNotice] = useState("");

  useEffect(() => {
    api.preferences().then(setPreferences).catch(() => setPreferences(defaults));
  }, []);
  useEffect(() => {
    const system = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const choice = preferences?.theme ?? "system";
      document.documentElement.dataset.theme = choice === "system" ? (system.matches ? "dark" : "light") : choice;
    };
    apply();
    system.addEventListener("change", apply);
    return () => system.removeEventListener("change", apply);
  }, [preferences?.theme]);

  if (!preferences) return <main className="boot"><Brand /><span role="status">Opening Zibblu…</span></main>;

  async function setTheme(theme: Theme) {
    await api.setTheme(theme);
    setPreferences(current => ({ ...(current ?? defaults), theme }));
  }

  if (session) {
    return <Workspace session={session} theme={preferences.theme} setTheme={setTheme}
      privateQuiet={preferences.private_quiet} setPrivateQuiet={async quiet => {
        await api.setPrivateQuiet(quiet);
        setPreferences(current => ({ ...(current ?? defaults), private_quiet: quiet }));
      }} signedOut={message => {
      setNotice(message ?? "");
      setSession(undefined);
    }} />;
  }
  return <SignIn initialServer={preferences.server} notice={notice} signedIn={result => {
    setPreferences({ ...preferences, server: result.server });
    setSession(result);
  }} />;
}

function SignIn({ initialServer, notice, signedIn }: {
  initialServer: string; notice: string; signedIn: (session: Session) => void;
}) {
  const [register, setRegister] = useState(false);
  const [server, setServer] = useState(initialServer);
  const [handle, setHandle] = useState("");
  const [password, setPassword] = useState("");
  const [failure, setFailure] = useState(notice);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
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
      <div>
        <h2>{register ? "Create an account" : "Sign in to Zibblu"}</h2>
        <p className="muted">{register ? "Choose a username friends can use to find you." : "Use your account and your Zibblu server address."}</p>
        <form onSubmit={submit}>
          <label className="floating-field">
            <input type="url" value={server} onChange={event => setServer(event.target.value)} required placeholder=" " />
            <span>Server address</span>
          </label>
          <label className="floating-field">
            <input value={handle} onChange={event => setHandle(event.target.value)} required
              autoComplete="username" maxLength={32} placeholder=" " />
            <span>Username</span>
          </label>
          <label className="floating-field">
            <input type="password" value={password} onChange={event => setPassword(event.target.value)} required
              autoComplete={register ? "new-password" : "current-password"} minLength={register ? 12 : undefined}
              maxLength={256} placeholder=" " />
            <span>Password</span>
          </label>
          {failure && <div className="inline-error" role="alert">{failure}</div>}
          <button className="button primary signin-submit" disabled={busy} type="submit">
            {busy ? "Connecting…" : register ? "Create account" : "Sign in"} <ArrowRight size={17} />
          </button>
        </form>
        <div className="signin-switch">{register ? "Already have an account?" : "New to Zibblu?"}{" "}
          <button className="text-button" onClick={() => { setRegister(!register); setFailure(""); }} disabled={busy}>
            {register ? "Sign in" : "Create an account"}
          </button>
        </div>
      </div>
    </section>
  </main>;
}

const invitation = getCurrentWindow().label === "private-invitation";
createRoot(document.getElementById("root")!).render(<DesktopMenus>
  {invitation ? <PrivateInvitationWindow /> : <App />}
</DesktopMenus>);
