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
  // 当前 30 秒窗口的 TOTP 验证码（watcher 每次轮询重新生成）：
  // 值随窗口变化，fillField 检测到字段里的旧码不同就会自动重输
  const TOTP = __TOTP__;
  // 手机号登录（phone 步骤填，值=云端账号 bind_phone）
  const PHONE = __PHONE__;
  // 最新短信验证码（watcher 每轮从服务端 api_phone_sms.php 实时拉取；
  // 短信未到时为空串，sms 步骤自动跳过，1.5s 后下一轮再试）
  const SMSCODE = __SMSCODE__;

  const host = location.hostname.toLowerCase();
  const path = location.pathname.toLowerCase();
  // Google 两步流是确定的：identifier 页没有密码/2FA 框、challenge 页没有
  // 邮箱框——这些「本来就不在本页」的 no-field 安静跳过，不算问题
  const isGoogle =
    host.includes("accounts.google.") || host.includes("accounts.youtube.");
  const isChallenge = path.includes("challenge") || path.includes("pwd");

  const ROLES = {
    email: EMAIL,
    password: PASSWORD,
    "2fa": TOTP,
    phone: PHONE,
    sms: SMSCODE,
  };

  // 我方已写入的字段登记表（el → 写入时的值）：window 级 WeakMap 跨轮
  // 保留（脚本每 1.5s 重注入但同一页面 window 不变）。用途：用户手改后
  // 绝不覆盖；TOTP 换码/新短信码这类我方主动刷新也要先核对旧值是我们写的
  const FILLED = window.__af_filled || (window.__af_filled = new WeakMap());

  // 返回状态：filled=写入成功并验证 / same=已是目标值 / hidden=不可见 /
  // disabled=禁用 / bounced=被受控组件弹回 / partial=只写入了一部分 /
  // user-modified=我方填过后被用户手改（尊重用户，不再碰）
  function fillField(el, value) {
    if (el.disabled) return "disabled";
    // 隐藏/未渲染的输入框（页面切换瞬间残留的别步骤字段）不输入
    if (el.offsetParent === null && el.getClientRects().length === 0) {
      return "hidden";
    }
    const stamped = FILLED.get(el);
    if (stamped !== undefined) {
      if (el.value !== stamped) {
        // 值和我写入时不一样 → 用户手动改过：绝不覆盖回去
        return "user-modified";
      }
      if (el.value === value) return "same";
      // 还是我们写的旧值，但目标值变了（TOTP 30 秒换码/新短信码）→ 继续走写入
    } else if (el.value === value) {
      return "same";
    }
    // React-style controlled inputs ignore plain assignment; the native
    // prototype setter plus a bubbled input event is what they listen to.
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    ).set;
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    // Vue/Element Plus 等受控组件可能立即把值弹回（逗哥实测每轮都弹）：
    // 换浏览器真实输入管线重试——execCommand insertText 走默认编辑行为，
    // 触发的是 trusted input 事件，Element/Vue/React 全认
    if (el.value !== value) {
      try {
        el.focus();
        el.select();
        document.execCommand("insertText", false, value);
        el.dispatchEvent(new Event("input", { bubbles: true }));
      } catch (e) {}
    }
    // 写入后验证真实值：不许把「以为填了」谎报成 filled
    if (el.value === value) {
      FILLED.set(el, value);
      return "filled";
    }
    if (el.value === "") return "bounced";
    return el.value.length < value.length ? "partial" : "bounced";
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

    // 用户明确要求（2026-11-24）：绝不自动点击任何按钮——「发送」验证码
    // 按钮由用户手动点，这里只填手机号、等短信、填验证码
    if (step.role === "sms-send") {
      skipped.push("sms-send:manual-click-required");
      continue;
    }

    const value = ROLES[step.role];
    if (value === undefined || value === "") {
      // 短信还没到是正常态（刚点发送），安静跳过不打噪音
      if (step.role !== "sms") skipped.push(`${step.role}-no-value`);
      continue;
    }
    // 分体式验证码框（PayPal 6 格 OTP 等）：step.split.length = 格数，
    // selectors 里第一条命中 >= length 个元素的逐格选择器生效，
    // 第 N 个格子填验证码第 N 位。fillField 走同一条输入管线（原生
    // setter + input/change 事件），每格输入后 React 会自动跳下一格。
    if (step.split && step.split.length > 0) {
      const n = step.split.length;
      let boxes = null;
      let hit = "";
      for (const selector of selectors) {
        const list = document.querySelectorAll(selector);
        if (list.length >= n) {
          boxes = Array.from(list).slice(0, n);
          hit = selector;
          break;
        }
      }
      if (!boxes) {
        // 页面上格子不够（不在 OTP 步 / 别步骤）按 no-field 上报
        skipped.push(`no-${step.role}-field(split<${n})`);
      } else if (value.length !== n) {
        // 验证码位数与格数不符（如 TOTP 密钥配置异常），不硬塞
        skipped.push(`${step.role}-length-mismatch(${value.length}!=${n})`);
      } else {
        let allOk = true;
        const states = [];
        for (let i = 0; i < n; i++) {
          const state = fillField(boxes[i], value[i]);
          states.push(state);
          if (state !== "filled" && state !== "same") allOk = false;
        }
        if (states.every((s) => s === "same")) {
          skipped.push(`${step.role}-already-filled(${hit})`);
        } else if (allOk) {
          filled.push(`${step.role}-split${n}(${hit})`);
        } else {
          // 有格子隐藏/禁用：整体算失败并带上逐格状态，方便对着改选择器
          skipped.push(
            `${step.role}-split-partial(${hit}:${states.join("/")})`,
          );
        }
      }
      continue;
    }
    // 逐个选择器试到命中为止；命中/不可见明细进 verdict，
    // 调试时对着 platform_config 改选择器即可
    const absent =
      isGoogle &&
      ((step.role === "email" && isChallenge) ||
        (step.role === "password" && isChallenge && !path.includes("pwd")) ||
        (step.role === "2fa" &&
          isChallenge &&
          !path.includes("totp") &&
          !path.includes("otp")));
    let el = null;
    let hit = "";
    for (const selector of selectors) {
      el = document.querySelector(selector);
      if (el) {
        hit = selector;
        break;
      }
    }
    if (!el) {
      skipped.push(
        absent ? `${step.role}-not-on-this-step` : `no-${step.role}-field`,
      );
    } else if (
      step.role === "email" &&
      (el.type === "password" ||
        el.type === "tel" ||
        (el.getAttribute("autocomplete") || "").includes("one-time-code") ||
        /totp|otp/i.test(`${el.name || ""} ${el.id || ""}`))
    ) {
      // 角色守卫：邮箱角色永不写 password / 验证码框。Google 的密码框、
      // 2-Step Verification 的验证码框（#totpPin，type=tel）都可能共用
      // jsname="YPqjbf"，不守卫会把登录账号打进验证码框
      skipped.push(`email-type-guard(${hit})`);
    } else {
      const state = fillField(el, value);
      if (state === "filled") {
        filled.push(`${step.role}(${hit})`);
      } else if (state === "same") {
        skipped.push(`${step.role}-already-filled(${hit})`);
      } else if (state === "user-modified") {
        // 用户手改过：安静跳过，不进 needs_attention（尊重用户输入）
        skipped.push(`${step.role}-user-modified(${hit})`);
      } else if (state === "bounced" || state === "partial") {
        // 被受控组件弹回/只写入一部分：值得大声报，对着选择器和组件改
        skipped.push(`${step.role}-bounced(${hit}:${state})`);
      } else if (state === "hidden") {
        // 隐藏框 + 本就不该在本页的步骤 = 两步流的正常状态，安静跳过
        skipped.push(
          absent
            ? `${step.role}-not-on-this-step`
            : `${step.role}-hidden(${hit})`,
        );
      } else {
        skipped.push(`${step.role}-disabled(${hit})`);
      }
    }
  }

  // 命中方案签名（首条规则），verdict 一眼看出命中的是哪条 matching 规则
  const rule = (plan.matching || [])[0] || {};
  const ruleSig = (rule.hosts || []).length
    ? `hosts=${rule.hosts.join(",")}`
    : `suffix=${(rule.host_suffixes || []).join(",")}`;
  // 填上的之外附带跳过明细：一条日志同时看到「填了什么」和「另一条腿为什么没填」
  const skippedPart = skipped.length ? ` |skipped:${skipped.join("+")}` : "";

  return filled.length > 0
    ? `filled:${filled.join("+")}${skippedPart} @${ruleSig}`
    : `filled:nothing:${skipped.join("+")}:on=${path} @${ruleSig}`;
})();
