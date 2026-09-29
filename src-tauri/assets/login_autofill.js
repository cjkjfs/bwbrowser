// Fills a profile's stored credentials into the sign-in pages described by the
// injected plans (pulled from platform_config, with a built-in Google/YouTube
// fallback). Injected by the login-autofill watcher on every probe. Idempotent:
// a field that already holds the credential is left alone. Never submits the
// form — the human still presses Next / Sign in, and a captcha or 2FA step
// simply waits for them. The __AUTOFILL_PLANS__ / __EMAIL__ / __PASSWORD__
// placeholders are substituted at script generation time.
(() => {
  const PLANS = __AUTOFILL_PLANS__;
  const EMAIL = __EMAIL__;
  const PASSWORD = __PASSWORD__;

  const host = location.hostname.toLowerCase();
  const path = location.pathname.toLowerCase();

  const ROLES = { email: EMAIL, password: PASSWORD };

  function fillField(el, value) {
    if (!el || el.disabled) return false;
    if (el.value === value) return false;
    // React-style controlled inputs ignore plain assignment; the native
    // prototype setter plus a bubbled input event is what they listen to.
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    ).set;
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  }

  function ruleMatches(rule) {
    const hosts = rule.hosts || [];
    const suffixes = rule.host_suffixes || [];
    const paths = rule.paths || [];
    const hostOk =
      (hosts.length === 0 && suffixes.length === 0) ||
      hosts.some((h) => host === h) ||
      suffixes.some((s) => host === s.replace(/^\./, "") || host.endsWith(s));
    const pathOk =
      paths.length === 0 || paths.some((p) => path === p || path.includes(p));
    return hostOk && pathOk;
  }

  const plan = (PLANS || []).find((p) => (p.matching || []).some(ruleMatches));
  if (!plan) return "skip:not-signin";

  const filled = [];
  const skipped = [];

  for (const step of plan.steps || []) {
    const selectors = step.selectors || [];
    const value = ROLES[step.role];
    if (value === undefined || value === "") {
      skipped.push(`${step.role}-no-value`);
      continue;
    }
    let el = null;
    for (const selector of selectors) {
      el = document.querySelector(selector);
      if (el) break;
    }
    if (!el) {
      skipped.push(`no-${step.role}-field`);
    } else if (fillField(el, value)) {
      filled.push(step.role);
    } else {
      skipped.push(`${step.role}-already-filled`);
    }
  }

  return filled.length > 0
    ? `filled:${filled.join("+")}`
    : `filled:nothing:${skipped.join("+")}:on=${path}`;
})();
