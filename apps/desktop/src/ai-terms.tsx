import { useEffect, useState } from "react";
import { ai } from "./ai-api";
import type { AiTerms } from "./ai-api";
import { errorMessage } from "./api";
import { Dialog } from "./common";
import { actions, t } from "./locale";
import { windows } from "./window-chrome";

export const aiTermsLabel = () => t("AI terms and privacy", "AI 服务条款与隐私说明");

export function useAiTerms() {
  const [reviewing, setReviewing] = useState(false);
  return {
    async requireAcceptance() {
      if (!windows || (await ai.terms()).accepted) return true;
      setReviewing(true);
      return false;
    },
    review: () => setReviewing(true),
    dialog: reviewing ? <AiTermsDialog close={() => setReviewing(false)} /> : null,
  };
}

export function AiTermsDialog({ close }: { close: () => void }) {
  const [terms, setTerms] = useState<AiTerms>();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  useEffect(() => {
    let active = true;
    void ai.terms().then(value => { if (active) setTerms(value); })
      .catch(error => { if (active) setFailure(errorMessage(error)); });
    return () => { active = false; };
  }, []);

  async function accept() {
    if (!terms || terms.accepted || busy) return;
    setBusy(true);
    setFailure("");
    try { await ai.acceptTerms(terms.text); close(); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  return <Dialog title={aiTermsLabel()} className="ai-terms-dialog" close={close} closeDisabled={busy}>
    {terms ? <>
      <div className="ai-terms-text" tabIndex={0} role="region" aria-label={aiTermsLabel()}>
        {terms.text.trim().split(/\r?\n\r?\n/).map((paragraph, index) => <p key={index}>{paragraph}</p>)}
      </div>
      <div className="dialog-actions">
        <button className="button subtle" type="button" disabled={busy} onClick={close}>
          {terms.accepted ? t("Close", "关闭") : actions.cancel}
        </button>
        {!terms.accepted && <button className="button primary" type="button" disabled={busy} onClick={() => void accept()}>
          {t("Accept terms", "接受条款")}
        </button>}
      </div>
    </> : !failure && <p role="status">{t("Loading terms…", "正在加载条款…")}</p>}
    {failure && <p className="inline-error" role="alert">{failure}</p>}
  </Dialog>;
}
