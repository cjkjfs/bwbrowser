// Fills a profile's stored credentials into Google/YouTube sign-in pages.
// Injected by the login-autofill watcher on every probe. Idempotent: a field
// that already holds the credential is left alone. Never submits the form —
// the human still presses Next / Sign in, and a captcha or 2FA step simply
// waits for them. The __EMAIL__ / __PASSWORD__ placeholders are substituted
// with JSON-escaped values when the script is generated.
(() => {
  const EMAIL = __EMAIL__;
  const PASSWORD = __PASSWORD__;

  const host = location.hostname.toLowerCase();
  const path = location.pathname.toLowerCase();
  const isSigninPage =
    host.endsWith("accounts.google.com") ||
    host.endsWith("accounts.youtube.com") ||
    ((host === "youtube.com" || host.endsWith(".youtube.com")) &&
      path.includes("/signin"));
  if (!isSigninPage) return "skip:not-signin";

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

  const filled = [];
  const skipped = [];

  // Email step: the visible identifier field. Google renders a hidden
  // duplicate with the same name, so hidden inputs are excluded.
  const emailEl = document.querySelector(
    'input[type="email"]:not([type="hidden"]), input[name="identifier"]:not([type="hidden"]), input[jsname="KKx9x"]:not([type="hidden"])',
  );
  if (!emailEl) {
    skipped.push("no-email-field");
  } else if (fillField(emailEl, EMAIL)) {
    filled.push("email");
  } else {
    skipped.push("email-already-filled");
  }

  // Password step: `Passwd` is the stable selector; fall back to any visible
  // password input that is not the security-code field.
  const passEl = document.querySelector(
    'input[name="Passwd"], input[type="password"]:not([name="ca"])',
  );
  if (!passEl) {
    skipped.push("no-password-field");
  } else if (fillField(passEl, PASSWORD)) {
    filled.push("password");
  } else {
    skipped.push("password-already-filled");
  }

  return filled.length > 0
    ? "filled:" + filled.join("+")
    : "filled:nothing:" + skipped.join("+") + ":on=" + path;
})();
