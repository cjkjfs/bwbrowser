import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { appFromEnvironment, withApp } from "../lib/app.mjs";

test("fresh app renders, completes onboarding, persists settings, and never touches real app roots", async () => {
  await withApp(
    "smoke-fresh",
    async (app) => {
      assert.equal(typeof (await app.session.title()), "string");
      assert.match(await app.bodyText(), /New/);
      await app.waitForText("No profiles yet");

      const initial = await app.invoke("get_app_settings");
      assert.equal(typeof initial.onboarding_completed, "boolean");
      await app.invoke("complete_onboarding");
      assert.equal(await app.invoke("get_onboarding_completed"), true);
      await app.invoke("dismiss_window_resize_warning");
      assert.equal(
        await app.invoke("get_window_resize_warning_dismissed"),
        true,
      );

      // Where the app draws its own titlebar it also owns the window controls,
      // so it needs the desktop's button layout to know which side they go on.
      const decorations = await app.invoke("get_window_decoration_layout");
      assert.equal(typeof decorations?.client_side, "boolean");
      if (decorations.client_side) {
        // Only reported where decorations were actually dropped, which is
        // every Linux session except KDE on Wayland.
        assert.equal(process.platform, "linux");
        // `layout` may be null when GtkSettings is unavailable; the frontend
        // falls back to the default arrangement rather than drawing nothing,
        // so asserting a string here would be stricter than the contract.
        if (decorations.layout !== null) {
          assert.equal(typeof decorations.layout, "string");
          assert.match(
            decorations.layout,
            /close|minimize|maximize/,
            `layout must name a drawable control, got: ${decorations.layout}`,
          );
        }
      } else {
        // The platform still draws a titlebar; the app must not draw a second.
        assert.equal(decorations.layout, null);
      }

      const saved = await app.invoke("save_app_settings", {
        settings: {
          ...initial,
          theme: "dark",
          language: "en",
          onboarding_completed: true,
          disable_auto_updates: true,
        },
      });
      assert.equal(saved.theme, "dark");
      assert.equal(saved.language, "en");

      await app.invoke("save_table_sorting_settings", {
        sorting: { column: "browser", direction: "desc" },
      });
      assert.deepEqual(await app.invoke("get_table_sorting_settings"), {
        column: "browser",
        direction: "desc",
      });
      assert.ok((await app.invoke("get_system_language")).length >= 2);
      const system = await app.invoke("get_system_info");
      assert.ok(system && typeof system === "object");
      assert.equal(typeof (await app.invoke("read_log_files")), "string");

      // Feature tips: what was seen, the one-a-day pacing, and the decision
      // behind the paid-plan welcome all live in the settings file.
      const tips = await app.invoke("get_tips_state");
      assert.equal(tips.auto_show, true);
      assert.deepEqual(tips.seen, []);
      assert.equal(tips.auto_due, true, "a fresh install owes its first tip");
      const marked = await app.invoke("mark_tip_seen", {
        tipId: "dnsBlocklist",
        auto: true,
      });
      assert.deepEqual(marked.seen, ["dnsBlocklist"]);
      assert.equal(typeof marked.last_auto_shown_at, "number");
      assert.equal(marked.auto_due, false, "one automatic tip a day");
      const browsed = await app.invoke("mark_tip_seen", {
        tipId: "proxyCheck",
        auto: false,
      });
      assert.deepEqual(browsed.seen, ["dnsBlocklist", "proxyCheck"]);
      assert.equal(
        browsed.last_auto_shown_at,
        marked.last_auto_shown_at,
        "a browsed tip must not restart the pacing",
      );
      const quiet = await app.invoke("set_tips_auto_show", { enabled: false });
      assert.equal(quiet.auto_show, false);
      assert.equal(quiet.auto_due, false);
      assert.equal(
        await app.invoke("observe_cloud_plan", {
          userId: "acct-free",
          paid: false,
          freshLogin: true,
        }),
        false,
        "a free account is never greeted",
      );
      assert.equal(
        await app.invoke("observe_cloud_plan", {
          userId: "acct-free",
          paid: true,
          freshLogin: false,
        }),
        true,
        "free to paid is the upgrade the welcome exists for",
      );
      assert.equal(
        await app.invoke("observe_cloud_plan", {
          userId: "acct-free",
          paid: true,
          freshLogin: true,
        }),
        false,
        "and it is greeted once",
      );
      assert.equal(
        await app.invoke("observe_cloud_plan", {
          userId: "acct-web",
          paid: true,
          freshLogin: true,
        }),
        true,
        "a paid account first seen right after signing in came from checkout",
      );
      assert.equal(
        await app.invoke("observe_cloud_plan", {
          userId: "acct-old",
          paid: true,
          freshLogin: false,
        }),
        false,
        "a paid account in an old session is not new to its plan",
      );

      await app.restart();
      const afterRestart = await app.invoke("get_app_settings");
      assert.equal(afterRestart.theme, "dark");
      assert.equal(afterRestart.language, "en");
      assert.equal(afterRestart.onboarding_completed, true);
      assert.deepEqual(afterRestart.tips_seen, ["dnsBlocklist", "proxyCheck"]);
      assert.equal(afterRestart.tips_auto_show, false);
      assert.deepEqual((await app.invoke("get_tips_state")).seen, [
        "dnsBlocklist",
        "proxyCheck",
      ]);

      const settingsFile = path.join(
        app.dataRoot,
        "data",
        "settings",
        "app_settings.json",
      );
      const persisted = JSON.parse(await readFile(settingsFile, "utf8"));
      assert.equal(persisted.api_token, null);
      assert.equal(persisted.mcp_token, null);
    },
    { onboardingCompleted: false },
  );
});

test("two isolated sessions run concurrently and do not share frontend or backend state", async () => {
  const first = appFromEnvironment("smoke-isolation-a");
  const second = appFromEnvironment("smoke-isolation-b");
  try {
    await Promise.all([first.start(), second.start()]);
    const firstSettings = await first.invoke("get_app_settings");
    await first.invoke("save_app_settings", {
      settings: { ...firstSettings, theme: "dark", onboarding_completed: true },
    });
    const secondSettings = await second.invoke("get_app_settings");
    assert.equal(secondSettings.theme, "system");
    assert.notEqual(secondSettings.theme, "dark");

    await first.execute("localStorage.setItem('bwbrowser-e2e-only-a', 'yes');");
    assert.equal(
      await second.execute(
        "return localStorage.getItem('bwbrowser-e2e-only-a');",
      ),
      null,
      "native WebView data leaked across sessions",
    );
  } catch (error) {
    await Promise.all([first.capture("failure"), second.capture("failure")]);
    throw error;
  } finally {
    await Promise.all([first.close(), second.close()]);
  }
});

test("ai rewrite api config commands fail cleanly without a gateway session", async () => {
  await withApp("smoke-ai-api-config", async (app) => {
    // 配置数据只存服务器（云端后台与本程序共用一份）；离线时两个命令都必须
    // 在网关登录一步干净失败，而不是 panic 或返回半套数据。
    const getErr = await app.invokeError("ai_rewrite_get_api_config");
    assert.match(getErr, /登录/, "get names the missing gateway session");
    const saveErr = await app.invokeError("ai_rewrite_save_api_config", {
      config: {},
    });
    assert.match(saveErr, /登录/, "save names the missing gateway session");
  });
});

test("ai agent chat fails cleanly without a gateway session", async () => {
  await withApp("smoke-ai-agent", async (app) => {
    // 端点配置只存服务器；离线时对话命令必须在网关登录一步干净失败，
    // 返回 backend error code JSON，而不是 panic 或绕过代理直连外网。
    const err = await app.invokeError("ai_agent_chat", {
      messages: [{ role: "user", content: "你好" }],
    });
    let code = "";
    let message = "";
    try {
      const parsed = JSON.parse(err);
      code = parsed.code ?? "";
      message = parsed.params?.message ?? "";
    } catch {
      // 兼容非 JSON 的原始错误文本
    }
    if (code) {
      assert.equal(code, "AI_AGENT_CONFIG_UNAVAILABLE");
      assert.match(message, /登录/, "error names the missing gateway session");
    } else {
      assert.match(err, /登录/, "error names the missing gateway session");
    }
  });
});

test("ai rewrite running tasks reads the local runtime table without a session", async () => {
  await withApp("smoke-ai-running-tasks", async (app) => {
    // 进度表是本机内存状态：不依赖网关会话，冷启动必须返回空表而不是报错。
    const res = await app.invoke("ai_rewrite_running_tasks");
    assert.equal(res.success, true);
    assert.deepEqual(res.tasks, []);
    // 停止开关是本机内存状态：置位/复位都只是写标志位，无会话也必须成功。
    const stopped = await app.invoke("ai_rewrite_stop");
    assert.equal(stopped.success, true);
    // 本地 ASR 配置读写：默认关闭、往返保存后能读回同样内容。
    const localAsr = await app.invoke("ai_rewrite_get_local_asr");
    assert.equal(localAsr.enabled, false);
    assert.equal(localAsr.model, "small");
    const saved = await app.invoke("ai_rewrite_save_local_asr", {
      cfg: { enabled: true, pythonPath: "", model: "medium", language: "zh" },
    });
    assert.equal(saved.success, true);
    const localAsr2 = await app.invoke("ai_rewrite_get_local_asr");
    assert.equal(localAsr2.enabled, true);
    assert.equal(localAsr2.model, "medium");
    // 模型预下载：缓存检查在无 Python 的环境必须干净失败（点名 Python），
    // 有 Python 时返回结构化 cached 布尔；下载命令对未知模型名直接拒绝。
    try {
      const checked = await app.invoke("ai_rewrite_check_local_model", {
        model: "tiny",
      });
      assert.equal(typeof checked.cached, "boolean");
    } catch (e) {
      assert.match(String(e), /Python/, "check error names Python");
    }
    const dlErr = await app.invokeError("ai_rewrite_download_local_model", {
      model: "not-a-model",
    });
    assert.match(dlErr, /不支持/, "download rejects unknown model");

    // 本地转写环境：检测命令返回结构化结果（python/source/hasWhisper）；
    // 安装命令可启动或因环境互斥/平台限制报错，但必须可调用。
    const env = await app.invoke("ai_rewrite_check_local_env");
    assert.ok(
      typeof env.hasWhisper === "boolean",
      "env check returns hasWhisper boolean",
    );
    try {
      const started = await app.invoke("ai_rewrite_install_local_env");
      assert.equal(started.started, true);
    } catch (e) {
      assert.match(
        String(e),
        /安装/,
        "install error mentions installation state",
      );
    }
  });
});

test("keyboard command palette and major navigation surfaces are operable through native WebDriver", async () => {
  await withApp("smoke-ui", async (app) => {
    const modifier =
      process.platform === "darwin" ? { meta: true } : { ctrl: true };
    await app.waitFor(
      async () => {
        await app.pressShortcut({ key: "k", ...modifier });
        return app.execute(
          `return Boolean(document.querySelector("[cmdk-input][placeholder='Type a command or search...']"));`,
        );
      },
      { description: "open command palette" },
    );

    const input = await app.session.findCss("[cmdk-input]");
    await app.session.sendKeys(input, "settings");
    const body = await app.bodyText();
    assert.match(body, /Settings/i);

    // Exercise native WebDriver element marshalling and click, not just script execution.
    // Scoped to the open dialog on purpose: on Linux the app draws its own
    // titlebar, whose "Close window" control appears earlier in the DOM, and
    // clicking that would exercise the window lifecycle instead of the palette.
    const close = await app.execute(
      `const dialog = document.querySelector("[role='dialog']") ?? document;
       return [...dialog.querySelectorAll("button")].find(
        (button) => /close/i.test(button.getAttribute("aria-label") || button.textContent || "")
      ) ?? null;`,
    );
    if (close) {
      await app.session.click(close);
    } else {
      await app.pressShortcut({ key: "Escape" });
    }
  });
});

test("tray labels, hide-to-tray, and confirmed quit follow the native lifecycle", async () => {
  const app = appFromEnvironment("smoke-lifecycle");
  try {
    await app.start();
    await app.invoke("update_tray_menu", {
      showLabel: "Show Bwbrowser E2E",
      quitLabel: "Quit Bwbrowser E2E",
    });
    await app.invoke("hide_to_tray");
    assert.equal(
      typeof (await app.invoke("get_onboarding_completed")),
      "boolean",
    );

    await app.restart();
    const exitingSession = app.session;
    await app
      .execute(
        `window.__TAURI_INTERNALS__.invoke("confirm_quit").catch(() => {});
         return true;`,
      )
      .catch(() => {});
    await app.waitFor(
      async () => {
        try {
          await exitingSession.title();
          return false;
        } catch {
          return true;
        }
      },
      { timeoutMs: 10_000, description: "confirmed app exit" },
    );
    app.session = null;
    await exitingSession.close().catch(() => {});
  } catch (error) {
    await app.capture("failure");
    throw error;
  } finally {
    await app.close();
  }
});

test("the data directory can be moved to another folder and the choice survives a restart", async () => {
  await withApp(
    "smoke-data-root",
    async (app) => {
      // Every path below is inside this session's own temporary root. The
      // real installation is never a source or a destination here.
      const defaultRoot = path.join(app.dataRoot, "data");
      const pointerFile = path.join(app.dataRoot, "data-root.json");
      const destination = path.join(app.root, "moved-bwbrowser-data");

      const before = await app.invoke("get_data_root_info");
      assert.equal(before.active_path, defaultRoot);
      assert.equal(before.configured_path, null);
      assert.equal(before.restart_required, false);
      assert.equal(before.active_path_missing, false);
      assert.equal(before.overridden_by_environment, false);
      assert.ok(before.file_count > 0, "the seeded settings file is counted");
      assert.ok(before.size_bytes > 0, "the directory reports a real size");

      const profile = await app.invoke("create_browser_profile_new", {
        name: "Carried Across",
        browserStr: "wayfern",
        version: "150.0.7871.100",
        releaseType: "stable",
        proxyId: null,
        vpnId: null,
        wayfernConfig: { fingerprint: "{}" },
        groupId: null,
        ephemeral: false,
        dnsBlocklist: null,
        launchHook: null,
      });

      // Each refusal is its own code, because each one has a different fix.
      assert.match(
        await app.invokeError("move_data_root", { destination: defaultRoot }),
        /DATA_ROOT_SAME_AS_CURRENT/,
      );
      assert.match(
        await app.invokeError("move_data_root", {
          destination: path.join(defaultRoot, "profiles", "elsewhere"),
        }),
        /DATA_ROOT_DESTINATION_INSIDE_SOURCE/,
      );
      assert.match(
        await app.invokeError("move_data_root", {
          destination: "not/absolute",
        }),
        /DATA_ROOT_DESTINATION_NOT_WRITABLE/,
      );
      assert.equal(
        existsSync(destination),
        false,
        "a refused move must not create the destination",
      );

      const moved = await app.invoke("move_data_root", { destination });
      assert.equal(moved.configured_path, destination);
      assert.equal(moved.restart_required, true);
      // The move takes effect at the next start: this process keeps every
      // path it resolved when it started.
      assert.equal(moved.active_path, defaultRoot);

      // Copy, then verify, then delete: the old directory only goes once the
      // copy has been proven whole.
      assert.equal(existsSync(defaultRoot), false, "the source is removed");
      assert.ok(
        existsSync(path.join(destination, "settings", "app_settings.json")),
        "settings travelled with the move",
      );
      assert.ok(
        existsSync(path.join(destination, "profiles")),
        "profiles travelled with the move",
      );

      // The pointer lives beside the data directory, never inside it, or the
      // delete above would have taken it and the next start would forget.
      const pointer = JSON.parse(await readFile(pointerFile, "utf8"));
      assert.equal(pointer.path, destination);

      await app.restart();

      const after = await app.invoke("get_data_root_info");
      assert.equal(after.active_path, destination);
      assert.equal(after.configured_path, destination);
      assert.equal(after.restart_required, false);
      assert.equal(after.active_path_missing, false);

      const profiles = await app.invoke("list_browser_profiles");
      assert.ok(
        profiles.some((entry) => entry.id === profile.id),
        "the moved directory still holds the profile",
      );
      const settings = await app.invoke("get_app_settings");
      assert.equal(settings.onboarding_completed, true);

      // Forgetting the choice is the escape hatch for a drive that is gone
      // for good; it moves nothing, so it too only lands on the next start.
      const cleared = await app.invoke("clear_data_root_choice");
      assert.equal(cleared.configured_path, null);
      assert.equal(existsSync(pointerFile), false);

      await app.restart();
      const restored = await app.invoke("get_data_root_info");
      assert.equal(restored.active_path, defaultRoot);
      assert.equal(restored.configured_path, null);

      // set_data_root: choosing a directory records it as the active root
      // immediately without moving anything, and keeps the previous root as the
      // migration source until move_data_root relocates it.
      const chosen = await app.invoke("set_data_root", { destination });
      assert.equal(chosen.configured_path, destination);
      assert.equal(chosen.active_path, destination);
      assert.equal(chosen.previous_path, defaultRoot);

      // Pointing back at the platform default hides the waiting-data hint:
      // the previous source is the active root again, so there is nothing
      // distinct left to migrate.
      const restore = await app.invoke("set_data_root", {
        destination: defaultRoot,
      });
      assert.equal(restore.active_path, defaultRoot);
      assert.equal(restore.previous_path, null);
    },
    { seedDownloadedBrowser: true },
  );
});
