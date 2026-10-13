<?php
/**
 * ai_rewrite.php — 爆文库 AI 洗稿工具
 *
 * 极简三步：① 选视频 ② 一键生成 ③ 拿文案
 * 已处理过的视频会自动复用历史转写结果，不重复调用 API。
 */

error_reporting(E_ALL);
ini_set('display_errors', 0);
ini_set('log_errors', 1);
if (!is_dir(__DIR__ . '/logs')) { @mkdir(__DIR__ . '/logs', 0755, true); }
ini_set('error_log', __DIR__ . '/logs/ai_rewrite_error.log');
set_time_limit(0);
ini_set('memory_limit', '512M');

require_once 'config.php';
require_once 'company_config.php';
require_once 'ai_rewrite_lib.php';

check_auth();
$user = get_logged_user($pdo);
if (empty($user)) { header('Location: login.php'); exit; }

$company_id      = get_current_company_id($pdo);
$is_super_admin  = is_super_admin($pdo);
$is_manager      = is_company_admin($pdo);
$user_id         = (int)$user['id'];
$system_logo     = function_exists('get_system_logo') ? get_system_logo($pdo) : '爆文库';

$module_perms = get_current_user_module_permissions($pdo, $user);
$can_access   = $is_super_admin || !empty($module_perms['allow_baowenku']);

// 超级管理员可切换公司视角
$view_company_id = $company_id;
if ($is_super_admin) {
    $req_cid = $_POST['company_id'] ?? $_GET['company_id'] ?? 0;
    if (is_numeric($req_cid) && (int)$req_cid > 0) { $view_company_id = (int)$req_cid; }
}

$companies_list = [];
if ($is_super_admin) {
    try { $companies_list = $pdo->query("SELECT id, company_name FROM companies ORDER BY company_name")->fetchAll(); }
    catch (Exception $e) { $companies_list = []; }
}

// ============================================================
// AJAX 接口
// ============================================================
$action = $_POST['action'] ?? '';
if ($action !== '') {
    header('Content-Type: application/json; charset=utf-8');
    function aijson($d) { echo json_encode($d, JSON_UNESCAPED_UNICODE); exit; }

    if (!$can_access) { aijson(['success' => false, 'message' => '无权限']); }

    try {
        ai_rw_ensure_tables($pdo);
        $cid = $view_company_id;

        switch ($action) {

            // ---------- 配置 ----------
            case 'config_get': {
                $cfg = ai_rw_get_config($pdo, $cid);
                aijson(['success' => true, 'config' => $cfg, 'ready' => ai_rw_config_ready($cfg)]);
            }

            case 'config_save': {
                $data = [];
                foreach (['asr_provider','asr_base_url','asr_api_key','asr_model',
                          'llm_provider','llm_base_url','llm_api_key','llm_model',
                          'analyze_prompt','rewrite_prompt','score_prompt',
                          'translate_model','translate_base_url','translate_api_key','translate_prompt',
                          'asr_proxy','asr_proxy_type','asr_proxy_user','asr_proxy_pass',
                          'llm_proxy','llm_proxy_type','llm_proxy_user','llm_proxy_pass'] as $f) {
                    if (isset($_POST[$f])) { $data[$f] = (string)$_POST[$f]; }
                }
                ai_rw_save_config($pdo, $cid, $data, $user_id);
                aijson(['success' => true, 'message' => '配置已保存']);
            }

            // ---------- 任务列表 ----------
            case 'task_list': {
                $filter = trim((string)($_POST['filter'] ?? ''));  // '' | pending | done | fail
                $keyword = trim((string)($_POST['keyword'] ?? ''));
                $page = max(1, (int)($_POST['page'] ?? 1));
                $page_size = min(100, max(5, (int)($_POST['page_size'] ?? 20)));
                $offset = ($page - 1) * $page_size;

                $where = ["company_id = ?"]; $params = [$cid];
                if ($filter === 'pending') { $where[] = "(asr_status IN (0,1) OR analyze_status IN (0,1) OR rewrite_status IN (0,1))"; }
                elseif ($filter === 'done') { $where[] = "asr_status=2 AND analyze_status=2 AND rewrite_status=2"; }
                elseif ($filter === 'fail') { $where[] = "(asr_status=3 OR analyze_status=3 OR rewrite_status=3)"; }
                if ($keyword !== '') { $where[] = "(title LIKE ? OR video_id LIKE ?)"; $params[] = '%'.$keyword.'%'; $params[] = '%'.$keyword.'%'; }
                $wsql = implode(' AND ', $where);

                $c = $pdo->prepare("SELECT COUNT(*) FROM ai_rewrite_tasks WHERE $wsql");
                $c->execute($params); $total = (int)$c->fetchColumn();

                $st = $pdo->prepare("SELECT id, source, video_id, platform, category, title, source_url, cover_url,
                                            play_count, asr_status, analyze_status, rewrite_status, retry_count, error_msg, created_at
                                     FROM ai_rewrite_tasks WHERE $wsql
                                     ORDER BY id DESC LIMIT $page_size OFFSET $offset");
                $st->execute($params);
                $rows = $st->fetchAll(PDO::FETCH_ASSOC);
                foreach ($rows as &$r) {
                    $r['play_count'] = (int)$r['play_count'];
                    $r['asr_status'] = (int)$r['asr_status'];
                    $r['analyze_status'] = (int)$r['analyze_status'];
                    $r['rewrite_status'] = (int)$r['rewrite_status'];
                    $r['done'] = ($r['asr_status'] === 2 && $r['analyze_status'] === 2 && $r['rewrite_status'] === 2);
                }
                unset($r);
                aijson(['success' => true, 'total' => $total, 'page' => $page, 'page_size' => $page_size, 'rows' => $rows]);
            }

            // ---------- 任务详情 ----------
            case 'task_detail': {
                $task_id = (int)($_POST['task_id'] ?? 0);
                $task = ai_rw_get_task($pdo, $task_id, $cid);
                if (!$task) { aijson(['success' => false, 'message' => '任务不存在']); }
                $analysis = $task['analyze_json'] ? ai_rw_extract_json((string)$task['analyze_json']) : null;
                $rewrite  = $task['rewrite_json'] ? ai_rw_extract_json((string)$task['rewrite_json']) : null;
                $scripts = $pdo->prepare("SELECT id, title, category, hook, body, cta, tags, kept_points, full_text, status, created_at
                                          FROM ai_scripts WHERE task_id = ? ORDER BY id DESC");
                $scripts->execute([$task_id]);
                aijson([
                    'success' => true,
                    'task' => [
                        'id' => (int)$task['id'], 'title' => $task['title'], 'platform' => $task['platform'],
                        'category' => $task['category'], 'source_url' => $task['source_url'], 'cover_url' => $task['cover_url'],
                        'play_count' => (int)$task['play_count'], 'video_id' => $task['video_id'],
                        'asr_text' => (string)$task['asr_text'], 'error_msg' => $task['error_msg'],
                        'asr_text_zh' => (string)($task['asr_text_zh'] ?? ''),
                        'rewrite_text' => (string)($task['rewrite_text'] ?? ''),
                        'rewrite_text_zh' => (string)($task['rewrite_text_zh'] ?? ''),
                        'reused_from' => (int)$task['reused_from'], 'asr_manual' => (int)$task['asr_manual'],
                        'asr_status' => (int)$task['asr_status'], 'analyze_status' => (int)$task['analyze_status'],
                        'rewrite_status' => (int)$task['rewrite_status'],
                    ],
                    'analysis' => $analysis,
                    'rewrite'  => $rewrite,
                    'scripts'  => $scripts->fetchAll(PDO::FETCH_ASSOC),
                ]);
            }

            // ---------- 删除任务 ----------
            case 'task_delete': {
                $task_id = (int)($_POST['task_id'] ?? 0);
                $task = ai_rw_get_task($pdo, $task_id, $cid);
                if (!$task) { aijson(['success' => false, 'message' => '任务不存在']); }
                $pdo->prepare("DELETE FROM ai_rewrite_tasks WHERE id = ?")->execute([$task_id]);
                $pdo->prepare("DELETE FROM viral_points WHERE task_id = ?")->execute([$task_id]);
                $pdo->prepare("DELETE FROM ai_scripts WHERE task_id = ?")->execute([$task_id]);
                aijson(['success' => true, 'message' => '已删除']);
            }

            // ---------- 删除爆款核心（不影响任务记录本身） ----------
            case 'point_delete': {
                $point_id = (int)($_POST['point_id'] ?? 0);
                $st = $pdo->prepare("DELETE FROM viral_points WHERE id = ? AND company_id = ?");
                $st->execute([$point_id, $cid]);
                aijson(['success' => true, 'message' => '已删除']);
            }

            // ---------- 爆款核心库 ----------
            case 'point_list': {
                $keyword = trim((string)($_POST['keyword'] ?? ''));
                $category = trim((string)($_POST['category'] ?? ''));
                $page = max(1, (int)($_POST['page'] ?? 1));
                $page_size = min(100, max(5, (int)($_POST['page_size'] ?? 20)));
                $offset = ($page - 1) * $page_size;

                $where = ["vp.company_id = ?"]; $params = [$cid];
                if ($keyword !== '') { $where[] = "(vp.hook LIKE ? OR vp.reusable LIKE ? OR vp.keywords LIKE ? OR vp.summary LIKE ?)"; for ($i=0;$i<4;$i++) $params[] = '%'.$keyword.'%'; }
                if ($category !== '') { $where[] = "vp.category = ?"; $params[] = $category; }
                $wsql = implode(' AND ', $where);

                $c = $pdo->prepare("SELECT COUNT(*) FROM viral_points vp WHERE $wsql");
                $c->execute($params); $total = (int)$c->fetchColumn();

                // 热度 = 播放量对数 × 时间衰减（90 天半衰期）：榜首永远是"最近的爆款"，不是去年的老套路
                // current_play_count = 账号采集表里该视频的最新播放量（入库快照可能过时）
                $heat = "(LOG10(vp.play_count + 1) * POW(0.5, DATEDIFF(NOW(), vp.created_at) / 90))";
                $st = $pdo->prepare("SELECT vp.id, vp.task_id, vp.video_id, vp.platform, vp.category, vp.work, vp.title,
                                            vp.source_url, vp.play_count, vp.hook, vp.structure_json, vp.points_json,
                                            vp.emotion, vp.keywords, vp.reusable, vp.summary, vp.created_at,
                                            ROUND($heat, 2) AS heat, COALESCE(v.cur_play, 0) AS current_play_count
                                     FROM viral_points vp
                                     LEFT JOIN (SELECT company_id, video_id, MAX(play_count) AS cur_play
                                                FROM videos GROUP BY company_id, video_id) v
                                            ON v.company_id = vp.company_id AND v.video_id = vp.video_id
                                     WHERE $wsql ORDER BY $heat DESC, vp.play_count DESC, vp.id DESC LIMIT $page_size OFFSET $offset");
                $st->execute($params);
                $rows = $st->fetchAll(PDO::FETCH_ASSOC);
                foreach ($rows as &$r) {
                    $r['play_count'] = (int)$r['play_count'];
                    $r['current_play_count'] = (int)$r['current_play_count'];
                    $r['structure'] = $r['structure_json'] ? json_decode($r['structure_json'], true) : [];
                    $r['points'] = $r['points_json'] !== null ? explode("\n", (string)$r['points_json']) : [];
                }
                unset($r);
                aijson(['success' => true, 'total' => $total, 'page' => $page, 'page_size' => $page_size, 'rows' => $rows]);
            }

            // ---------- 文案库 ----------
            case 'script_list': {
                $keyword = trim((string)($_POST['keyword'] ?? ''));
                $page = max(1, (int)($_POST['page'] ?? 1));
                $page_size = min(100, max(5, (int)($_POST['page_size'] ?? 20)));
                $offset = ($page - 1) * $page_size;

                $where = ["company_id = ?"]; $params = [$cid];
                if ($keyword !== '') { $where[] = "(title LIKE ? OR full_text LIKE ?)"; $params[] = '%'.$keyword.'%'; $params[] = '%'.$keyword.'%'; }
                $wsql = implode(' AND ', $where);

                $c = $pdo->prepare("SELECT COUNT(*) FROM ai_scripts WHERE $wsql");
                $c->execute($params); $total = (int)$c->fetchColumn();

                $st = $pdo->prepare("SELECT id, task_id, category, title, hook, body, cta, tags, kept_points, full_text,
                                            score, score_json, scored_at, status, created_at
                                     FROM ai_scripts WHERE $wsql ORDER BY id DESC LIMIT $page_size OFFSET $offset");
                $st->execute($params);
                $rows = $st->fetchAll(PDO::FETCH_ASSOC);
                foreach ($rows as &$r) {
                    $r['score'] = ($r['score'] === null) ? null : (int)$r['score'];
                    $r['score_data'] = $r['score_json'] ? ai_rw_extract_json((string)$r['score_json']) : null;
                    unset($r['score_json']);
                }
                unset($r);
                aijson(['success' => true, 'total' => $total, 'page' => $page, 'page_size' => $page_size, 'rows' => $rows]);
            }

            // ---------- 文案打分（0-100） ----------
            case 'score_run': {
                $script_id = (int)($_POST['script_id'] ?? 0);
                if ($script_id <= 0) { aijson(['success' => false, 'message' => '缺少文案ID']); }

                $st = $pdo->prepare("SELECT * FROM ai_scripts WHERE id = ? AND company_id = ? LIMIT 1");
                $st->execute([$script_id, $cid]);
                $script = $st->fetch(PDO::FETCH_ASSOC);
                if (!$script) { aijson(['success' => false, 'message' => '文案不存在']); }

                $cfg = ai_rw_get_config($pdo, $cid);
                if (!ai_rw_config_ready($cfg)) {
                    aijson(['success' => false, 'need_config' => true, 'message' => 'AI 配置未完成：请先在「高级设置 → AI 接口配置」填写 LLM API Key']);
                }

                $text = trim((string)$script['full_text']);
                if ($text === '') { aijson(['success' => false, 'message' => '该文案内容为空，无法打分']); }

                $res = ai_rw_run_score($cfg, $text, [
                    'category' => (string)$script['category'],
                    'title'    => (string)$script['title'],
                ]);
                if (empty($res['ok'])) { aijson(['success' => false, 'message' => $res['error']]); }

                $score = ai_rw_save_score($pdo, $script_id, $res['data']);
                aijson(['success' => true, 'script_id' => $script_id, 'score' => $score, 'data' => $res['data']]);
            }

            case 'script_delete': {
                $sid = (int)($_POST['script_id'] ?? 0);
                $st = $pdo->prepare("DELETE FROM ai_scripts WHERE id = ? AND company_id = ?");
                $st->execute([$sid, $cid]);
                aijson(['success' => true, 'message' => '已删除']);
            }

            // ---------- 翻译（原文 / 洗稿 → 中文，供人工对照核查） ----------
            case 'translate': {
                $task_id = (int)($_POST['task_id'] ?? 0);
                $scope   = trim((string)($_POST['scope'] ?? 'both'));   // asr | rewrite | both
                $force   = ((int)($_POST['force'] ?? 0)) === 1;
                $target  = trim((string)($_POST['target'] ?? ''));
                if ($target === '') { $target = '中文'; }

                $task = ai_rw_get_task($pdo, $task_id, $cid);
                if (!$task) { aijson(['success' => false, 'message' => '任务不存在']); }
                $cfg = ai_rw_get_config($pdo, $cid);

                $out = ['asr_text_zh' => '', 'rewrite_text_zh' => ''];
                $errs = [];

                if ($scope === 'asr' || $scope === 'both') {
                    $src   = trim((string)$task['asr_text']);
                    $exist = trim((string)($task['asr_text_zh'] ?? ''));
                    if ($src === '') { if ($scope === 'asr') { $errs[] = '原文转写为空，无法翻译'; } }
                    elseif ($exist !== '' && !$force) { $out['asr_text_zh'] = $exist; }
                    else {
                        $r = ai_rw_run_translate($cfg, $src, $target);
                        if (empty($r['ok'])) { $errs[] = '原文翻译失败：' . $r['error']; }
                        else { ai_rw_save_translation($pdo, $task_id, 'asr_text_zh', $r['text']); $out['asr_text_zh'] = $r['text']; }
                    }
                }
                if ($scope === 'rewrite' || $scope === 'both') {
                    $src   = trim((string)($task['rewrite_text'] ?? ''));
                    $exist = trim((string)($task['rewrite_text_zh'] ?? ''));
                    if ($src === '') { if ($scope === 'rewrite') { $errs[] = '洗稿文案为空，无法翻译'; } }
                    elseif ($exist !== '' && !$force) { $out['rewrite_text_zh'] = $exist; }
                    else {
                        $r = ai_rw_run_translate($cfg, $src, $target);
                        if (empty($r['ok'])) { $errs[] = '洗稿翻译失败：' . $r['error']; }
                        else { ai_rw_save_translation($pdo, $task_id, 'rewrite_text_zh', $r['text']); $out['rewrite_text_zh'] = $r['text']; }
                    }
                }

                $ok = ($out['asr_text_zh'] !== '' || $out['rewrite_text_zh'] !== '');
                aijson([
                    'success' => $ok,
                    'message' => $ok ? '翻译完成' : (implode('；', $errs) ?: '暂无可翻译的内容'),
                    'data'    => $out,
                    'errors'  => $errs,
                ]);
            }

            default:
                aijson(['success' => false, 'message' => '未知操作：' . $action]);
        }
    } catch (Exception $e) {
        aijson(['success' => false, 'message' => '服务器错误：' . $e->getMessage()]);
    }
}

// ============================================================
// 页面
// ============================================================
if (!$can_access) {
    ?><!DOCTYPE html><html lang="zh-CN"><head><meta charset="UTF-8"><title>无权限</title></head>
    <body><div style="text-align:center;padding:80px;color:#999;">无权限访问此页面（需要「爆文库」权限）</div></body></html><?php
    exit;
}
?>
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AI 洗稿工具 - <?php echo htmlspecialchars($system_logo); ?></title>
    <?php echo get_header_styles(); ?>
    <style>
        * { margin:0; padding:0; box-sizing:border-box; }
        body { background:#f5f7fb; font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif; color:#333; }
        .main-wrapper { margin:0 auto; padding:0 24px; }
        .container { width:100%; max-width:1360px; margin:0 auto; padding-bottom:80px; }
        .nav-tabs { display:flex; gap:10px; margin-bottom:24px; background:white; padding:8px 16px; border-radius:12px; flex-wrap:wrap; }
        .nav-tabs a { padding:10px 20px; text-decoration:none; color:#666; border-radius:8px; }
        .nav-tabs a.active, .nav-tabs a:hover { background:#667eea; color:white; }

        /* 顶部说明 */
        .hero { background:linear-gradient(135deg,#667eea,#764ba2); color:#fff; border-radius:16px; padding:26px 28px; margin:18px 0 22px; }
        .hero h1 { font-size:22px; margin-bottom:8px; }
        .hero p { font-size:14px; opacity:.92; line-height:1.7; }
        .hero .co { margin-top:12px; font-size:13px; }
        .hero select { padding:6px 10px; border-radius:8px; border:none; font-size:13px; }

        /* 步骤卡片 */
        .step { background:#fff; border-radius:16px; padding:22px 24px; margin-bottom:18px; box-shadow:0 2px 10px rgba(0,0,0,.05); }
        .step-head { display:flex; align-items:center; gap:14px; margin-bottom:16px; }
        .step-num { flex:0 0 auto; width:34px; height:34px; border-radius:50%; background:linear-gradient(135deg,#667eea,#764ba2); color:#fff; display:flex; align-items:center; justify-content:center; font-weight:700; font-size:16px; }
        .step-head h2 { font-size:17px; }
        .step-head .sub { font-size:13px; color:#8a8f9c; margin-top:3px; font-weight:400; }

        /* 模式切换 */
        .seg { display:inline-flex; background:#eef1f8; border-radius:10px; padding:4px; gap:4px; margin-bottom:16px; }
        .seg button { border:none; background:transparent; padding:9px 20px; border-radius:8px; cursor:pointer; font-size:14px; color:#5a6070; font-weight:500; }
        .seg button.on { background:#fff; color:#4b53d6; box-shadow:0 1px 4px rgba(0,0,0,.12); }

        .filters { display:flex; gap:10px; flex-wrap:wrap; align-items:center; margin-bottom:14px; }
        .filters select, .filters input { padding:8px 10px; border:1px solid #dfe3ec; border-radius:8px; font-size:13px; }
        .filters input[type=text] { min-width:170px; }
        .btn { padding:9px 18px; border:none; border-radius:9px; background:#667eea; color:#fff; cursor:pointer; font-size:13.5px; font-weight:500; }
        .btn:hover { opacity:.92; }
        .btn.gray { background:#eef1f6; color:#444; }
        .btn.green { background:#22c55e; }
        .btn.orange { background:#f59e0b; }
        .btn.sm { padding:5px 11px; font-size:12px; }
        .btn:disabled { opacity:.5; cursor:not-allowed; }

        table { width:100%; border-collapse:collapse; }
        th,td { padding:9px 8px; text-align:left; border-bottom:1px solid #f1f3f7; font-size:13px; vertical-align:middle; }
        th { background:#f8f9fc; color:#666; font-weight:600; white-space:nowrap; }
        tr:hover td { background:#fafbff; }
        .thumb { width:46px; height:60px; object-fit:cover; border-radius:6px; background:#eee; }
        .muted { color:#98a0b0; font-size:12px; }
        .badge { display:inline-block; padding:2px 9px; border-radius:10px; font-size:11px; white-space:nowrap; }
        .b-gray { background:#eef1f6; color:#777; } .b-blue { background:#e0e9ff; color:#3355cc; }
        .b-green { background:#dcfce7; color:#15803d; } .b-red { background:#fee2e2; color:#b91c1c; }
        .b-orange { background:#fef3c7; color:#b45309; } .b-purple { background:#ede9fe; color:#6d28d9; }
        .empty { text-align:center; color:#aab; padding:26px; font-size:13px; }
        .pager { display:flex; gap:8px; align-items:center; justify-content:center; margin-top:14px; }

        .field { margin-bottom:12px; }
        .field label { display:block; font-size:12px; color:#666; margin-bottom:5px; }
        .field input, .field textarea, .field select { width:100%; padding:9px 10px; border:1px solid #dfe3ec; border-radius:8px; font-size:13px; font-family:inherit; }
        .field textarea { resize:vertical; min-height:90px; }
        .grid2 { display:grid; grid-template-columns:1fr 1fr; gap:12px; }

        /* 生成按钮区 */
        .gen-bar { display:flex; align-items:center; gap:16px; flex-wrap:wrap; }
        .gen-btn { padding:14px 34px; font-size:16px; border-radius:12px; background:linear-gradient(135deg,#22c55e,#16a34a); color:#fff; border:none; cursor:pointer; font-weight:600; box-shadow:0 4px 14px rgba(34,197,94,.3); }
        .gen-btn:disabled { opacity:.6; cursor:not-allowed; box-shadow:none; }
        .gen-tip { font-size:13px; color:#8a8f9c; }
        .notice { margin-top:14px; padding:12px 16px; border-radius:10px; font-size:13.5px; line-height:1.7; display:none; }
        .notice.on { display:block; }
        .notice.info { background:#eef2ff; color:#3b45b8; }
        .notice.warn { background:#fff7e6; color:#b45309; }
        .notice.err  { background:#fee2e2; color:#b91c1c; }

        /* 结果卡片 */
        .res-card { border:1px solid #eceff5; border-radius:14px; padding:16px 18px; margin-bottom:14px; }
        .res-head { display:flex; gap:12px; align-items:flex-start; }
        .res-head .cover { width:56px; height:74px; object-fit:cover; border-radius:8px; background:#eee; flex:0 0 auto; }
        .res-head .info { flex:1; min-width:0; }
        .res-head .info .t { font-size:14.5px; font-weight:600; line-height:1.5; word-break:break-word; }
        .res-head .info .m { margin-top:5px; font-size:12px; color:#98a0b0; }
        .res-status { margin-top:12px; font-size:13px; color:#8a8f9c; }
        .spin { display:inline-block; width:13px; height:13px; border:2px solid #c7cdf5; border-top-color:#667eea; border-radius:50%; animation:sp .7s linear infinite; vertical-align:-2px; margin-right:6px; }
        @keyframes sp { to { transform:rotate(360deg); } }
        .script-box { margin-top:12px; background:#f8f9fd; border:1px solid #eef0f7; border-radius:10px; padding:14px 16px; font-size:14px; line-height:1.9; white-space:pre-wrap; word-break:break-word; }
        .script-label { font-size:12px; color:#667eea; font-weight:600; margin:12px 0 6px; }
        .steps-line { display:flex; gap:10px; flex-wrap:wrap; margin-top:10px; font-size:12px; }
        .steps-line span { padding:3px 10px; border-radius:20px; background:#eef1f6; color:#777; }
        .steps-line span.ok { background:#dcfce7; color:#15803d; }
        .steps-line span.now { background:#e0e9ff; color:#3355cc; }
        .steps-line span.bad { background:#fee2e2; color:#b91c1c; }
        .res-actions { margin-top:12px; display:flex; gap:8px; flex-wrap:wrap; }

        /* 打分 */
        .score-box { margin-top:12px; border:1px solid #eef0f7; border-radius:10px; padding:12px 14px; background:#fbfcff; }
        .score-top { display:flex; align-items:center; gap:8px; font-size:14px; }
        .score-sum { margin-top:8px; font-size:13.5px; color:#444; line-height:1.7; }
        .score-dims { margin-top:10px; }
        .dim { display:flex; align-items:center; gap:8px; font-size:12.5px; margin-top:7px; }
        .dim-name { width:112px; color:#666; flex:0 0 auto; }
        .dim-bar { flex:1; height:8px; background:#e9edf7; border-radius:6px; overflow:hidden; }
        .dim-bar i { display:block; height:100%; background:linear-gradient(90deg,#667eea,#764ba2); }
        .dim-sc { width:34px; text-align:right; color:#555; }
        .dim-cmt { font-size:12px; color:#98a0b0; margin:3px 0 0 120px; }
        .score-sug { margin-top:12px; }
        .score-sug-t { font-size:12.5px; color:#b45309; font-weight:600; margin-bottom:4px; }
        .score-sug ul { margin:0; padding-left:20px; }
        .score-sug li { font-size:13px; line-height:1.85; color:#444; }

        /* 高级设置 */
        .adv { background:#fff; border-radius:16px; padding:6px 24px; box-shadow:0 2px 10px rgba(0,0,0,.05); }
        .adv > summary { cursor:pointer; padding:16px 0; font-size:14px; font-weight:600; color:#5a6070; list-style:none; }
        .adv > summary::-webkit-details-marker { display:none; }
        .adv > summary::before { content:'⚙️ '; }
        .adv-body { padding-bottom:22px; }
        .subtabs { display:flex; gap:8px; flex-wrap:wrap; margin-bottom:16px; }
        .subtabs button { padding:8px 16px; border:none; background:#f1f3f9; border-radius:9px; cursor:pointer; font-size:13px; color:#5a6070; }
        .subtabs button.on { background:#667eea; color:#fff; }
        .subpanel { display:none; } .subpanel.on { display:block; }

        /* 弹窗 */
        .modal-mask { display:none; position:fixed; inset:0; background:rgba(0,0,0,.45); z-index:999; }
        .modal-mask.on { display:flex; align-items:flex-start; justify-content:center; padding:40px 16px; overflow:auto; }
        .modal { background:#fff; border-radius:14px; width:min(1040px,96vw); padding:22px; }
        .modal h3 { font-size:16px; margin-bottom:12px; }
        .modal .close { float:right; cursor:pointer; color:#999; font-size:20px; line-height:1; }
        pre.box { background:#f7f8fb; border:1px solid #eee; border-radius:8px; padding:12px; font-size:12.5px; white-space:pre-wrap; word-break:break-word; max-height:340px; overflow:auto; }
        .toast { position:fixed; top:20px; right:20px; background:#333; color:#fff; padding:12px 18px; border-radius:10px; font-size:13px; z-index:2000; opacity:0; transition:opacity .25s; max-width:340px; }
        .toast.on { opacity:1; }
        .row-actions { display:flex; gap:6px; flex-wrap:wrap; }
        @media (max-width:640px){ .grid2 { grid-template-columns:1fr; } }
    </style>
</head>
<body>
<?php render_header($pdo, $user); ?>
<div class="main-wrapper">
    <?php include 'nav.php'; ?>
    <div class="container">

    <div class="hero">
        <h1>✍️ AI 洗稿 · 记录与配置</h1>
        <p>视频下载、口播转写、爆点分析、洗稿、打分由 <b>BW Browser 客户端</b>在本地完成（网页服务器做不了转写与洗稿）。<br>
           本页面用于<b>查看历史记录</b>（文案 / 任务 / 爆款核心）和<b>配置 AI 接口</b>（下发给客户端使用）。</p>
        <?php if ($is_super_admin && !empty($companies_list)): ?>
        <div class="co">
            当前公司：
            <select id="companySwitch" onchange="switchCompany(this.value)">
                <?php foreach ($companies_list as $co): ?>
                <option value="<?php echo (int)$co['id']; ?>" <?php echo (int)$co['id'] === (int)$view_company_id ? 'selected' : ''; ?>>
                    <?php echo htmlspecialchars($co['company_name']); ?> (#<?php echo (int)$co['id']; ?>)
                </option>
                <?php endforeach; ?>
            </select>
        </div>
        <?php endif; ?>
    </div>

    <div class="subtabs" style="margin-bottom:16px;">
        <button class="on" data-sub="scripts" onclick="subTab('scripts')">📄 文案记录</button>
        <button data-sub="tasks" onclick="subTab('tasks')">🗂 全部任务</button>
        <button data-sub="points" onclick="subTab('points')">🔥 爆款核心库</button>
        <button data-sub="cfg" onclick="subTab('cfg')">⚙️ AI 接口配置</button>
    </div>

    <!-- ========== 文案记录（历史成果） ========== -->
    <div class="subpanel on" id="sub-scripts">
    <div class="step">
        <div class="step-head">
            <div class="step-num">📄</div>
            <div>
                <h2>文案记录</h2>
                <div class="sub">客户端生成的文案都入库在这里 → 可查看、打分（0–100 分，AI 给出评分维度与改进建议）、删除</div>
            </div>
        </div>
        <div class="filters">
            <input type="text" id="sKeyword" placeholder="搜索标题 / 正文" onkeydown="if(event.key==='Enter')loadScripts(1)">
            <button class="btn" onclick="loadScripts(1)">查询</button>
            <button class="btn green" id="scoreBatchBtn" onclick="scoreSelected()">🎯 打分选中</button>
            <span class="muted" id="scoreTip">已选 0 条</span>
        </div>
        <div style="overflow:auto;">
            <table>
                <thead><tr>
                    <th style="width:34px;"><input type="checkbox" id="sSelAll" onchange="toggleAllScripts(this)"></th>
                    <th>文案</th><th>赛道</th><th>评分</th><th>操作</th>
                </tr></thead>
                <tbody id="sBody"><tr><td colspan="5" class="empty">加载中…</td></tr></tbody>
            </table>
        </div>
        <div class="pager" id="sPager"></div>
    </div>
    </div>

    <!-- ========== 全部任务 ========== -->
    <div class="subpanel" id="sub-tasks">
        <div class="step">
        <div class="step-head">
            <div class="step-num">🗂</div>
            <div>
                <h2>全部任务</h2>
                <div class="sub">客户端上报的洗稿任务记录（含转写 / 分析 / 洗稿各步状态）</div>
            </div>
        </div>
        <div class="filters">
            <select id="tFilter">
                <option value="">全部</option>
                <option value="pending">未完成</option>
                <option value="done">已完成</option>
                <option value="fail">失败</option>
            </select>
            <input type="text" id="tKeyword" placeholder="搜索标题 / 视频ID">
            <button class="btn" onclick="loadTasks(1)">查询</button>
        </div>
        <div style="overflow:auto;">
            <table>
                <thead><tr><th>#</th><th>标题</th><th>平台</th><th>播放量</th><th>转写</th><th>分析</th><th>洗稿</th><th>操作</th></tr></thead>
                <tbody id="tBody"><tr><td colspan="8" class="empty">加载中…</td></tr></tbody>
            </table>
        </div>
        <div class="pager" id="tPager"></div>
        </div>
    </div>

    <!-- ========== 爆款核心库 ========== -->
    <div class="subpanel" id="sub-points">
        <div class="step">
        <div class="step-head">
            <div class="step-num">🔥</div>
            <div>
                <h2>爆款核心库</h2>
                <div class="sub">从已处理视频里沉淀的钩子、结构与爆点</div>
            </div>
        </div>
        <div class="filters">
            <input type="text" id="pKeyword" placeholder="搜索钩子 / 套路 / 关键词">
            <input type="text" id="pCategory" placeholder="赛道">
            <button class="btn" onclick="loadPoints(1)">查询</button>
        </div>
        <div style="overflow:auto;">
            <table>
                <thead><tr><th>#</th><th>作品</th><th>标题</th><th>赛道</th><th>播放量</th><th>热度</th><th>钩子</th><th>操作</th></tr></thead>
                <tbody id="pBody"><tr><td colspan="8" class="empty">加载中…</td></tr></tbody>
            </table>
        </div>
        <div class="pager" id="pPager"></div>
        </div>
    </div>

    <!-- ========== AI 接口配置 ========== -->
    <div class="subpanel" id="sub-cfg">
        <div class="step">
        <div class="step-head">
            <div class="step-num">⚙️</div>
            <div>
                <h2>AI 接口配置</h2>
                <div class="sub">这里的 ASR / LLM 配置会下发给 BW Browser 客户端使用</div>
            </div>
        </div>
        <div class="grid2">
            <div class="field"><label>ASR 接口地址 (Base URL)</label><input type="text" id="cAsrBase" placeholder="https://api.siliconflow.cn/v1"></div>
            <div class="field"><label>ASR 模型</label><input type="text" id="cAsrModel" placeholder="FunAudioLLM/SenseVoiceSmall"></div>
        </div>
        <div class="field"><label>ASR API Key</label><input type="text" id="cAsrKey" placeholder="sk-..."></div>
        <div class="grid2">
            <div class="field"><label>ASR 代理类型</label>
                <select id="cAsrProxyType"><option value="http">HTTP</option><option value="socks5">SOCKS5</option></select>
            </div>
            <div class="field"><label>ASR 代理地址 (host:port，可空=直连)</label><input type="text" id="cAsrProxy" placeholder="1.2.3.4:8080"></div>
        </div>
        <div class="grid2">
            <div class="field"><label>ASR 代理用户名（可空）</label><input type="text" id="cAsrProxyUser"></div>
            <div class="field"><label>ASR 代理密码（可空）</label><input type="text" id="cAsrProxyPass"></div>
        </div>
        <div class="muted" style="margin:2px 0 10px;">翻译请求跟随 ASR 代理线路。</div>
        <div class="grid2">
            <div class="field"><label>LLM 协议</label>
                <select id="cLlmProvider" onchange="updateProtoTip()">
                    <option value="openai_compatible">OpenAI 兼容（DeepSeek / 硅基流动等）</option>
                    <option value="anthropic">Claude（Anthropic 官方）</option>
                    <option value="gemini">Google Gemini</option>
                </select>
            </div>
            <div class="field"><label>LLM 模型</label><input type="text" id="cLlmModel" placeholder="deepseek-chat / claude-sonnet-4-5"></div>
        </div>
        <div class="grid2">
            <div class="field"><label>LLM 接口地址 (Base URL)</label><input type="text" id="cLlmBase" placeholder="https://api.deepseek.com/v1"></div>
            <div class="field"><label>LLM API Key</label><input type="text" id="cLlmKey" placeholder="sk-..."></div>
        </div>
        <div class="muted" id="llmProtoTip" style="margin:2px 0 10px;">选 Claude 官方时 Base URL 填 https://api.anthropic.com/v1，且必须填写代理线路（固定 IP，换 IP 可能封号）。</div>
        <div class="grid2">
            <div class="field"><label>LLM 代理类型</label>
                <select id="cLlmProxyType"><option value="http">HTTP</option><option value="socks5">SOCKS5</option></select>
            </div>
            <div class="field"><label>LLM 代理地址 (host:port，可空=直连)</label><input type="text" id="cLlmProxy" placeholder="1.2.3.4:8080"></div>
        </div>
        <div class="grid2">
            <div class="field"><label>LLM 代理用户名（可空）</label><input type="text" id="cLlmProxyUser"></div>
            <div class="field"><label>LLM 代理密码（可空）</label><input type="text" id="cLlmProxyPass"></div>
        </div>
        <div class="field"><label>爆点分析提示词</label><textarea id="cAnalyzePrompt" style="min-height:170px;"></textarea></div>
        <div class="field"><label>洗稿提示词</label><textarea id="cRewritePrompt" style="min-height:170px;"></textarea></div>
        <div class="field"><label>打分提示词</label><textarea id="cScorePrompt" style="min-height:170px;"></textarea></div>
        <div style="border-top:1px dashed #e3e7f0;margin:14px 0 12px;"></div>
        <div class="grid2">
            <div class="field"><label>翻译模型（免费）</label><input type="text" id="cTranslateModel" placeholder="tencent/Hunyuan-MT-7B"></div>
            <div class="field"><label>翻译接口地址（留空复用 ASR 地址）</label><input type="text" id="cTranslateBase" placeholder="留空则复用 ASR Base URL"></div>
        </div>
        <div class="field"><label>翻译 API Key（留空复用 ASR Key）</label><input type="text" id="cTranslateKey" placeholder="留空则复用 ASR API Key"></div>
        <div class="field"><label>翻译提示词（{目标语言} 为占位符）</label><textarea id="cTranslatePrompt" style="min-height:120px;"></textarea></div>
        <button class="btn green" onclick="saveConfig()">保存配置</button>
        <span class="muted" id="cfgTip" style="margin-left:10px;"></span>
        </div>
    </div>
    </div>
</div>

<div class="modal-mask" id="modalMask" onclick="if(event.target===this)closeModal()">
    <div class="modal" id="modalBody"></div>
</div>
<div class="toast" id="toast"></div>

<script>
var CID = <?php echo (int)$view_company_id; ?>;
var P_PAGE = 1, T_PAGE = 1, S_PAGE = 1, PAGE_SIZE = 20;

/* ---------- 基础工具 ---------- */
function toast(msg) {
    var t = document.getElementById('toast');
    t.textContent = msg; t.classList.add('on');
    clearTimeout(window.__tt); window.__tt = setTimeout(function(){ t.classList.remove('on'); }, 3000);
}
function esc(s){ return String(s==null?'':s).replace(/[&<>"']/g, function(c){ return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]; }); }
function fmtNum(n){ n = Number(n)||0; if(n>=1e8) return (n/1e8).toFixed(1)+'亿'; if(n>=1e4) return (n/1e4).toFixed(1)+'万'; return n.toLocaleString(); }
function platformBadge(p){
    var map = {tiktok:['TikTok','b-blue'], youtube:['YouTube','b-red'], douyin:['抖音','b-orange']};
    var x = map[p] || [p||'-','b-gray'];
    return '<span class="badge '+x[1]+'">'+x[0]+'</span>';
}
function stBadge(code){
    var map = {0:['待处理','b-gray'],1:['处理中','b-blue'],2:['完成','b-green'],3:['失败','b-red']};
    var x = map[code] || ['?','b-gray'];
    return '<span class="badge '+x[1]+'">'+x[0]+'</span>';
}
function post(action, params) {
    var fd = new URLSearchParams();
    fd.append('action', action);
    fd.append('company_id', CID);
    for (var k in params) {
        var v = params[k];
        if (Array.isArray(v)) { v.forEach(function(x){ fd.append(k+'[]', x); }); }
        else if (v !== undefined && v !== null) { fd.append(k, v); }
    }
    return fetch('ai_rewrite.php', { method:'POST', body:fd, credentials:'same-origin' })
        .then(function(r){ return r.text(); })
        .then(function(t){ try { return JSON.parse(t); } catch(e){ throw new Error('返回异常：'+t.slice(0,200)); } });
}
function switchCompany(id){ var u = new URL(location.href); u.searchParams.set('company_id', id); location.href = u.toString(); }
function composeScript(w) {
    var parts = [];
    if (w.hook) parts.push(w.hook);
    if (w.body) { if (Array.isArray(w.body)) parts = parts.concat(w.body); else parts.push(w.body); }
    if (w.cta) parts.push(w.cta);
    return parts.join('\n');
}

/* ---------- 第4步：文案打分 ---------- */
function scoreBadge(score){
    score = Number(score)||0;
    var cls = score>=90?'b-green':(score>=75?'b-blue':(score>=60?'b-orange':'b-red'));
    return '<span class="badge '+cls+'" style="font-size:14px;padding:3px 12px;font-weight:700;">'+score+' 分</span>';
}
function scoreHtml(data){
    if(!data) return '';
    var h = '<div class="score-box">';
    h += '<div class="score-top">'+scoreBadge(data.score)+' <b>'+esc(data.level||'')+'</b></div>';
    if(data.summary) h += '<div class="score-sum">'+esc(data.summary)+'</div>';
    if(Array.isArray(data.dimensions) && data.dimensions.length){
        h += '<div class="score-dims">';
        data.dimensions.forEach(function(d){
            var v = Math.max(0, Math.min(100, Number(d.score)||0));
            h += '<div class="dim"><span class="dim-name">'+esc(d.name||'')+'</span>'+
                 '<span class="dim-bar"><i style="width:'+v+'%"></i></span>'+
                 '<span class="dim-sc">'+v+'</span></div>';
            if(d.comment) h += '<div class="dim-cmt">'+esc(d.comment)+'</div>';
        });
        h += '</div>';
    }
    if(Array.isArray(data.suggestions) && data.suggestions.length){
        h += '<div class="score-sug"><div class="score-sug-t">💡 改进建议</div><ul>';
        data.suggestions.forEach(function(s){ h += '<li>'+esc(s)+'</li>'; });
        h += '</ul></div>';
    }
    h += '</div>';
    return h;
}
function loadScripts(page){
    S_PAGE = page || 1;
    post('script_list', { keyword: document.getElementById('sKeyword').value, page: S_PAGE, page_size: PAGE_SIZE })
        .then(function(r){
            if(!r.success){ toast(r.message||'加载失败'); return; }
            window.__scripts = {};
            var h = '';
            if(!r.rows.length) h = '<tr><td colspan="5" class="empty">还没有生成文案</td></tr>';
            r.rows.forEach(function(s){
                window.__scripts[s.id] = s;
                h += '<tr>'+
                     '<td><input type="checkbox" class="ssel" value="'+s.id+'" onchange="updateScoreTip()"></td>'+
                     '<td><div style="max-width:620px;font-weight:600;">'+esc(s.title||('文案 #'+s.id))+'</div>'+
                         '<div class="muted" style="max-width:620px;max-height:38px;overflow:hidden;">'+esc(String(s.full_text||'').replace(/\n/g,' '))+'</div></td>'+
                     '<td>'+esc(s.category||'-')+'</td>'+
                     '<td id="ssc-'+s.id+'">'+((s.score===null||s.score===undefined)?'<span class="badge b-gray">未评</span>':scoreBadge(s.score))+'</td>'+
                     '<td><div class="row-actions">'+
                         '<button class="btn sm" style="background:#7c3aed;" onclick="scoreScript('+s.id+')">🎯 打分</button>'+
                         '<button class="btn sm gray" onclick="toggleScript('+s.id+')">查看</button>'+
                         '<button class="btn sm" style="background:#ef4444;" onclick="delScript('+s.id+')">删除</button>'+
                     '</div></td></tr>';
                h += '<tr id="sdet-'+s.id+'" style="display:none;"><td></td><td colspan="4"><div id="sdetbox-'+s.id+'"></div></td></tr>';
            });
            document.getElementById('sBody').innerHTML = h;
            document.getElementById('sSelAll').checked = false;
            renderPager('sPager', r.total, r.page, r.page_size, loadScripts);
            updateScoreTip();
        }).catch(function(e){ toast(e.message); });
}
function updateScoreTip(){
    var n = document.querySelectorAll('.ssel:checked').length;
    document.getElementById('scoreTip').textContent = '已选 ' + n + ' 条';
}
function toggleAllScripts(cb){
    document.querySelectorAll('.ssel').forEach(function(x){ x.checked = cb.checked; });
    updateScoreTip();
}
function toggleScript(id){
    var row = document.getElementById('sdet-'+id);
    if(!row) return;
    if(row.style.display !== 'none'){ row.style.display='none'; return; }
    row.style.display='';
    var s = (window.__scripts||{})[id];
    var box = document.getElementById('sdetbox-'+id);
    if(!box || !s) return;
    var h = '';
    if(s.score_data) h += scoreHtml(s.score_data);
    h += '<div class="script-label">完整文案</div><div class="script-box">'+esc(s.full_text||'')+'</div>';
    box.innerHTML = h;
}
function scoreScript(id){
    var s = (window.__scripts||{})[id];
    if(!s){ toast('数据已刷新，请重新查询'); return Promise.resolve(); }
    var cell = document.getElementById('ssc-'+id);
    if(cell) cell.innerHTML = '<span class="spin"></span>';
    return post('score_run', { script_id: id }).then(function(r){
        if(!r.success){
            if(cell) cell.innerHTML = (s.score===null||s.score===undefined)?'<span class="badge b-gray">未评</span>':scoreBadge(s.score);
            toast(r.message||'打分失败');
            return;
        }
        s.score = r.score; s.score_data = r.data;
        if(cell) cell.innerHTML = scoreBadge(r.score);
        var row = document.getElementById('sdet-'+id);
        if(row) row.style.display='';
        var box = document.getElementById('sdetbox-'+id);
        if(box) box.innerHTML = scoreHtml(r.data)+'<div class="script-label">完整文案</div><div class="script-box">'+esc(s.full_text||'')+'</div>';
    }).catch(function(e){ toast(e.message); });
}
function scoreSelected(){
    var ids = Array.prototype.map.call(document.querySelectorAll('.ssel:checked'), function(x){ return Number(x.value); });
    if(!ids.length){ toast('请先勾选要打分的文案'); return; }
    var btn = document.getElementById('scoreBatchBtn');
    btn.disabled = true;
    var i = 0;
    function next(){
        if(i >= ids.length){ btn.disabled=false; updateScoreTip(); toast('全部打分完成'); return; }
        var id = ids[i++];
        document.getElementById('scoreTip').textContent = '正在打分 '+i+'/'+ids.length+'…';
        scoreScript(id).then(next).catch(next);
    }
    next();
}
function delScript(id){
    if(!confirm('确定删除文案 #'+id+'？')) return;
    post('script_delete', { script_id: id }).then(function(r){ toast(r.message||'已删除'); loadScripts(S_PAGE); }).catch(function(e){ toast(e.message); });
}

/* ---------- 页签切换 ---------- */
function subTab(name){
    document.querySelectorAll('.subtabs button').forEach(function(b){ b.classList.toggle('on', b.dataset.sub===name); });
    document.querySelectorAll('.subpanel').forEach(function(p){ p.classList.remove('on'); });
    document.getElementById('sub-'+name).classList.add('on');
    if (name==='cfg') loadConfig();
    if (name==='points') loadPoints(1);
    if (name==='tasks') loadTasks(1);
}

/* ---------- 配置 ---------- */
function loadConfig(){
    post('config_get', {}).then(function(r){
        if (!r.success) { toast(r.message||'失败'); return; }
        var c = r.config;
        document.getElementById('cAsrBase').value = c.asr_base_url||'';
        document.getElementById('cAsrModel').value = c.asr_model||'';
        document.getElementById('cAsrKey').value = c.asr_api_key||'';
        document.getElementById('cAsrProxyType').value = c.asr_proxy_type||'http';
        document.getElementById('cAsrProxy').value = c.asr_proxy||'';
        document.getElementById('cAsrProxyUser').value = c.asr_proxy_user||'';
        document.getElementById('cAsrProxyPass').value = c.asr_proxy_pass||'';
        document.getElementById('cLlmProvider').value = c.llm_provider||'openai_compatible';
        document.getElementById('cLlmBase').value = c.llm_base_url||'';
        document.getElementById('cLlmModel').value = c.llm_model||'';
        document.getElementById('cLlmKey').value = c.llm_api_key||'';
        document.getElementById('cLlmProxyType').value = c.llm_proxy_type||'http';
        document.getElementById('cLlmProxy').value = c.llm_proxy||'';
        document.getElementById('cLlmProxyUser').value = c.llm_proxy_user||'';
        document.getElementById('cLlmProxyPass').value = c.llm_proxy_pass||'';
        updateProtoTip();
        document.getElementById('cAnalyzePrompt').value = c.analyze_prompt||'';
        document.getElementById('cRewritePrompt').value = c.rewrite_prompt||'';
        document.getElementById('cScorePrompt').value = c.score_prompt||'';
        document.getElementById('cTranslateModel').value = c.translate_model||'';
        document.getElementById('cTranslateBase').value = c.translate_base_url||'';
        document.getElementById('cTranslateKey').value = c.translate_api_key||'';
        document.getElementById('cTranslatePrompt').value = c.translate_prompt||'';
        document.getElementById('cfgTip').textContent = r.ready ? '✅ 已配置可用' : '⚠ 尚未填写 Key，无法调用';
    }).catch(function(e){ toast(e.message); });
}
function updateProtoTip(){
    var v = document.getElementById('cLlmProvider').value;
    var tip = document.getElementById('llmProtoTip');
    if (v === 'anthropic') {
        tip.textContent = 'Claude 官方：Base URL 填 https://api.anthropic.com/v1，模型如 claude-sonnet-4-5。必须填写 LLM 代理线路（固定 IP，换 IP 可能封号），客户端与服务端都会强制走代理。';
    } else if (v === 'gemini') {
        tip.textContent = 'Gemini：Base URL 填 https://generativelanguage.googleapis.com/v1beta，模型如 gemini-2.0-flash。代理可选。';
    } else {
        tip.textContent = 'OpenAI 兼容：Base URL 填到 /v1（如 https://api.deepseek.com/v1）。代理可选。';
    }
}
function saveConfig(){
    post('config_save', {
        asr_base_url: document.getElementById('cAsrBase').value,
        asr_model: document.getElementById('cAsrModel').value,
        asr_api_key: document.getElementById('cAsrKey').value,
        asr_proxy_type: document.getElementById('cAsrProxyType').value,
        asr_proxy: document.getElementById('cAsrProxy').value,
        asr_proxy_user: document.getElementById('cAsrProxyUser').value,
        asr_proxy_pass: document.getElementById('cAsrProxyPass').value,
        llm_provider: document.getElementById('cLlmProvider').value,
        llm_base_url: document.getElementById('cLlmBase').value,
        llm_model: document.getElementById('cLlmModel').value,
        llm_api_key: document.getElementById('cLlmKey').value,
        llm_proxy_type: document.getElementById('cLlmProxyType').value,
        llm_proxy: document.getElementById('cLlmProxy').value,
        llm_proxy_user: document.getElementById('cLlmProxyUser').value,
        llm_proxy_pass: document.getElementById('cLlmProxyPass').value,
        analyze_prompt: document.getElementById('cAnalyzePrompt').value,
        rewrite_prompt: document.getElementById('cRewritePrompt').value,
        score_prompt: document.getElementById('cScorePrompt').value,
        translate_model: document.getElementById('cTranslateModel').value,
        translate_base_url: document.getElementById('cTranslateBase').value,
        translate_api_key: document.getElementById('cTranslateKey').value,
        translate_prompt: document.getElementById('cTranslatePrompt').value
    }).then(function(r){ toast(r.message||'已保存'); loadConfig(); }).catch(function(e){ toast(e.message); });
}

/* ---------- 爆款核心库 ---------- */
function loadPoints(page){
    P_PAGE = page || 1;
    post('point_list', { keyword: document.getElementById('pKeyword').value, category: document.getElementById('pCategory').value, page: P_PAGE, page_size: PAGE_SIZE })
        .then(function(r){
            if (!r.success) { toast(r.message||'加载失败'); return; }
            var h = '';
            if (!r.rows.length) h = '<tr><td colspan="8" class="empty">还没有沉淀爆款核心</td></tr>';
            window.__points = {};
            r.rows.forEach(function(p){
                window.__points[p.id] = p;
                var work = (p.work||'').split('|')[0];
                h += '<tr>' +
                     '<td>'+p.id+'</td>' +
                     '<td>'+(work?esc(work):'<span class="muted">-</span>')+'</td>' +
                     '<td><div style="max-width:280px;">'+esc(p.title||'-')+'</div><div class="muted">'+platformBadge(p.platform)+'</div></td>' +
                     '<td>'+esc(p.category||'-')+'</td>' +
                     '<td>'+fmtNum(p.play_count)+'</td>' +
                     '<td>'+(p.heat!=null?esc(p.heat):'-')+'</td>' +
                     '<td><div style="max-width:340px;" class="muted">'+esc(p.hook||'-')+'</div></td>' +
                     '<td><button class="btn sm gray" onclick="showPoint('+p.id+')">详情</button> ' +
                     '<button class="btn sm gray" onclick="delPoint('+p.id+')">删除</button></td>' +
                     '</tr>';
            });
            document.getElementById('pBody').innerHTML = h;
            renderPager('pPager', r.total, r.page, r.page_size, loadPoints);
        }).catch(function(e){ toast(e.message); });
}
function delPoint(id){
    if (!confirm('删除这条爆款核心？任务记录本身不受影响。')) return;
    post('point_delete', { point_id: id }).then(function(r){
        if (!r.success) { toast(r.message||'删除失败'); return; }
        toast('已删除');
        loadPoints(P_PAGE);
    }).catch(function(e){ toast(e.message); });
}
function showPoint(id){
    var p = (window.__points||{})[id];
    if (!p) { toast('数据已刷新，请重新打开'); return; }
    var h = '<span class="close" onclick="closeModal()">×</span>';
    h += '<h3>🔥 爆款核心 #'+p.id+'</h3>';
    h += '<div style="font-size:13px;line-height:1.9;"><b>标题：</b>'+esc(p.title||'-')+'<br><b>平台：</b>'+esc(p.platform||'-')+'　<b>赛道：</b>'+esc(p.category||'-')+'　<b>作品：</b>'+esc((p.work||'').split('|').join(' / ')||'-')+'　<b>入库时播放：</b>'+fmtNum(p.play_count)+(p.current_play_count>0?'　<b>最新采集播放：</b>'+fmtNum(p.current_play_count):'')+'<br><b>情绪：</b>'+esc(p.emotion||'-')+'<br><b>可复用套路：</b>'+esc(p.reusable||'-')+'<br><b>关键词：</b>'+esc(p.keywords||'-')+'</div>';
    h += '<div class="field" style="margin-top:12px;"><label>钩子</label><pre class="box">'+esc(p.hook||'-')+'</pre></div>';
    if (p.structure && p.structure.length) {
        var s = p.structure.map(function(x){ return '· ['+(x.stage||'')+'] '+(x.desc||''); }).join('\n');
        h += '<div class="field"><label>结构</label><pre class="box">'+esc(s)+'</pre></div>';
    }
    if (p.points && p.points.length) h += '<div class="field"><label>爆点</label><pre class="box">'+esc(p.points.join('\n'))+'</pre></div>';
    if (p.summary) h += '<div class="field"><label>一句话总结</label><pre class="box">'+esc(p.summary)+'</pre></div>';
    document.getElementById('modalBody').innerHTML = h;
    document.getElementById('modalMask').classList.add('on');
}

/* ---------- 全部任务 ---------- */
function loadTasks(page) {
    T_PAGE = page || 1;
    post('task_list', {
        filter: document.getElementById('tFilter').value,
        keyword: document.getElementById('tKeyword').value,
        page: T_PAGE, page_size: PAGE_SIZE
    }).then(function(r){
        if (!r.success) { toast(r.message||'加载失败'); return; }
        var h = '';
        if (!r.rows.length) h = '<tr><td colspan="8" class="empty">没有任务</td></tr>';
        r.rows.forEach(function(t){
            h += '<tr>' +
                 '<td>'+t.id+'</td>' +
                 '<td><div style="max-width:340px;">'+esc(t.title||'(无标题)')+'</div><div class="muted">'+esc(t.source)+' · '+esc(t.video_id)+'</div>' +
                     (t.error_msg? '<div style="color:#b91c1c;font-size:12px;max-width:340px;">⚠ '+esc(t.error_msg)+'</div>' : '') + '</td>' +
                 '<td>'+platformBadge(t.platform)+'</td>' +
                 '<td>'+fmtNum(t.play_count)+'</td>' +
                 '<td>'+stBadge(t.asr_status)+'</td>' +
                 '<td>'+stBadge(t.analyze_status)+'</td>' +
                 '<td>'+stBadge(t.rewrite_status)+'</td>' +
                 '<td><div class="row-actions">' +
                     '<button class="btn sm gray" onclick="showTask('+t.id+')">详情</button>' +
                     '<button class="btn sm" style="background:#ef4444;" onclick="delTask('+t.id+')">删除</button>' +
                 '</div></td></tr>';
        });
        document.getElementById('tBody').innerHTML = h;
        renderPager('tPager', r.total, r.page, r.page_size, loadTasks);
    }).catch(function(e){ toast(e.message); });
}
function delTask(id){
    if (!confirm('确定删除任务 #'+id+'？同时会删除它的爆款核心与文案记录。')) return;
    post('task_delete', { task_id: id }).then(function(r){ toast(r.message||'已删除'); loadTasks(T_PAGE); }).catch(function(e){ toast(e.message); });
}

/* ---------- 任务详情弹窗 ---------- */
function transCompareHtml(t){
    var hasAsr = (t.asr_text||'').trim() !== '';
    var hasRw  = (t.rewrite_text||'').trim() !== '';
    if (!hasAsr && !hasRw) return '<div class="muted" style="margin:6px 0;">暂无可翻译的内容</div>';
    var h = '';
    if (hasAsr) {
        h += '<div style="margin-top:10px;"><div class="script-label" style="margin-top:0;">🎧 原文转写</div>' +
             '<div class="grid2">' +
             '<pre class="box" style="max-height:260px;">'+esc(t.asr_text||'')+'</pre>' +
             '<pre class="box" style="max-height:260px;">'+esc(t.asr_text_zh||'（未翻译）')+'</pre>' +
             '</div>' +
             '<div class="grid2" style="margin-top:4px;"><div class="muted">原文</div><div class="muted">中文译文</div></div></div>';
    }
    if (hasRw) {
        h += '<div style="margin-top:14px;"><div class="script-label" style="margin-top:0;">📝 洗稿文案</div>' +
             '<div class="grid2">' +
             '<pre class="box" style="max-height:260px;">'+esc(t.rewrite_text||'')+'</pre>' +
             '<pre class="box" style="max-height:260px;">'+esc(t.rewrite_text_zh||'（未翻译）')+'</pre>' +
             '</div>' +
             '<div class="grid2" style="margin-top:4px;"><div class="muted">洗稿（与原语言一致）</div><div class="muted">中文译文</div></div></div>';
    }
    return h;
}
function showTask(id){
    post('task_detail', { task_id: id }).then(function(r){
        if (!r.success) { toast(r.message||'失败'); return; }
        var t = r.task, a = r.analysis, w = r.rewrite;
        window.__taskDetail = r;
        var h = '<span class="close" onclick="closeModal()">×</span>';
        h += '<h3>#'+t.id+' '+esc(t.title||'(无标题)')+'</h3>';
        h += '<div style="font-size:13px;margin-bottom:12px;">'+platformBadge(t.platform)+' 播放 '+fmtNum(t.play_count)+' · <a href="'+esc(t.source_url)+'" target="_blank" style="color:#667eea;">原视频</a> · 转写 '+stBadge(t.asr_status)+' 分析 '+stBadge(t.analyze_status)+' 洗稿 '+stBadge(t.rewrite_status)+'</div>';
        if (t.error_msg) h += '<div style="color:#b91c1c;margin-bottom:10px;">⚠ '+esc(t.error_msg)+'</div>';

        h += '<div class="field"><label>🌐 翻译对照（中文）</label>' +
             '<div class="res-actions" style="margin-top:0;">' +
             '<button class="btn sm" style="background:#0ea5e9;" onclick="translateTask('+t.id+',0)">🌐 译成中文</button>' +
             '<button class="btn sm gray" onclick="translateTask('+t.id+',1)">↻ 重新翻译</button>' +
             '<span class="muted">原文与洗稿都译成中文，方便人工核查</span>' +
             '</div>' +
             '<div id="transbox-'+t.id+'">'+transCompareHtml(t)+'</div>' +
             '</div>';

        h += '<div class="field"><label>① 口播转写（ASR）</label><pre class="box">'+esc(t.asr_text||'（暂无）')+'</pre></div>';
        if (a) h += '<div class="field"><label>② 爆款核心分析</label><pre class="box">'+esc(JSON.stringify(a, null, 2))+'</pre></div>';
        if (w) {
            h += '<div class="field"><label>③ 洗稿文案</label><pre class="box">'+esc(composeScript(w))+'</pre></div>';
        }
        if (r.scripts && r.scripts.length) {
            h += '<div class="field"><label>已入库文案（'+r.scripts.length+'）</label>';
            r.scripts.forEach(function(s){ h += '<pre class="box" style="max-height:200px;">'+esc(s.full_text||'')+'</pre>'; });
            h += '</div>';
        }
        document.getElementById('modalBody').innerHTML = h;
        document.getElementById('modalMask').classList.add('on');
    }).catch(function(e){ toast(e.message); });
}
function translateTask(id, force){
    var box = document.getElementById('transbox-'+id);
    if (box) box.innerHTML = '<div class="muted" style="margin:6px 0;"><span class="spin"></span>翻译中…（首次调用较慢，请稍候）</div>';
    post('translate', { task_id: id, scope: 'both', force: force ? 1 : 0 }).then(function(r){
        var d = window.__taskDetail;
        if (!r.success) {
            toast(r.message||'翻译失败');
            if (box) box.innerHTML = '<div style="color:#b91c1c;margin:6px 0;">⚠ '+esc(r.message||'翻译失败')+'</div>' + ((d && d.task && d.task.id===id) ? transCompareHtml(d.task) : '');
            return;
        }
        toast(r.message||'翻译完成');
        if (d && d.task && d.task.id===id) {
            d.task.asr_text_zh = (r.data.asr_text_zh || d.task.asr_text_zh || '');
            d.task.rewrite_text_zh = (r.data.rewrite_text_zh || d.task.rewrite_text_zh || '');
            if (box) box.innerHTML = transCompareHtml(d.task);
        }
    }).catch(function(e){
        toast(e.message);
        if (box) box.innerHTML = '<div style="color:#b91c1c;margin:6px 0;">⚠ '+esc(e.message)+'</div>';
    });
}
function closeModal(){ document.getElementById('modalMask').classList.remove('on'); }

/* ---------- 分页 ---------- */
function renderPager(id, total, page, size, fn){
    var pages = Math.max(1, Math.ceil(total/size));
    var el = document.getElementById(id);
    if (total === 0) { el.innerHTML = ''; return; }
    el.innerHTML = '<button class="btn sm gray" '+(page<=1?'disabled':'')+'>上一页</button>' +
        '<span class="muted">第 '+page+' / '+pages+' 页，共 '+total+' 条</span>' +
        '<button class="btn sm gray" '+(page>=pages?'disabled':'')+'>下一页</button>';
    var btns = el.querySelectorAll('button');
    btns[0].onclick = function(){ if(page>1) fn(page-1); };
    btns[1].onclick = function(){ if(page<pages) fn(page+1); };
}

/* ---------- 初始化 ---------- */
loadScripts(1);
</script>
</body>
</html>