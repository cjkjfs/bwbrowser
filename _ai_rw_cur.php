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

// 普通员工只能看到自己洗稿的记录；超管/公司管理员可看全公司（0 = 不按人过滤）
// 查看范围：所有人可见本公司全部洗稿记录（页面顶部按洗稿人 tab 筛选）；
// 管理动作（AI 接口配置/打分）仍限管理员；删除按行级所有权校验（可删自己的，管理员任意）
$rw_scope_uid = 0;
$rw_can_manage = ($is_super_admin || $is_manager);

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

        // 从代理池行提取高级协议完整 URI（与客户端 bwbrowser_to_proxy_settings 同逻辑）
        function ai_rw_proxy_uri($type, $protocol_config) {
            $t = strtolower(trim((string)$type));
            if (!in_array($t, ['vless','trojan'], true)) { return null; }
            $pc = trim((string)$protocol_config);
            if ($pc === '') { return null; }
            $j = json_decode($pc, true);
            if (is_array($j)) {
                $key = $t === 'vless' ? 'vless_url' : 'trojan_url';
                return (isset($j[$key]) && is_string($j[$key]) && $j[$key] !== '') ? $j[$key] : null;
            }
            return (stripos($pc, $t.'://') === 0) ? $pc : null;
        }

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
                          'translate_model','translate_base_url','translate_api_key','translate_prompt'] as $f) {
                    if (isset($_POST[$f])) { $data[$f] = (string)$_POST[$f]; }
                }
                // 网页代理选择：''=直连（整组清空）；数字=代理中心条目（展开写入）；__keep__=不动
                foreach (['asr','llm'] as $k) {
                    $pick = isset($_POST[$k.'_proxy_pick']) ? trim((string)$_POST[$k.'_proxy_pick']) : '__keep__';
                    if ($pick === '__keep__') { continue; }
                    if ($pick === '') {
                        $data[$k.'_proxy'] = ''; $data[$k.'_proxy_type'] = ''; $data[$k.'_proxy_user'] = ''; $data[$k.'_proxy_pass'] = '';
                        continue;
                    }
                    if (ctype_digit($pick)) {
                        $pq = $pdo->prepare("SELECT * FROM simprint_proxies WHERE id = ? AND company_id = ?");
                        $pq->execute([(int)$pick, $cid]);
                        if ($pp = $pq->fetch()) {
                            $pt = strtolower(trim((string)$pp['proxy_type']));
                            $uri = ai_rw_proxy_uri($pp['proxy_type'], $pp['protocol_config'] ?? null);
                            if ($uri !== null) {
                                $data[$k.'_proxy'] = $uri; $data[$k.'_proxy_type'] = $pt;
                                $data[$k.'_proxy_user'] = ''; $data[$k.'_proxy_pass'] = '';
                            } elseif (!in_array($pt, ['vless','trojan','ss'], true)) {
                                $data[$k.'_proxy'] = $pp['host'] . ':' . (int)$pp['port'];
                                $data[$k.'_proxy_type'] = ($pt === 'socks' || $pt === 'socks5') ? 'socks5' : 'http';
                                $data[$k.'_proxy_user'] = (string)($pp['username'] ?? '');
                                $data[$k.'_proxy_pass'] = (string)($pp['password'] ?? '');
                            }
                        }
                    }
                }
                ai_rw_save_config($pdo, $cid, $data, $user_id);
                aijson(['success' => true, 'message' => '配置已保存']);
            }

            // ---------- 测试模型连接（文案模型 / ASR / 翻译） ----------
            case 'llm_test': {
                if (!$rw_can_manage) { aijson(['success' => false, 'message' => '无权限']); }
                $group = trim((string)($_POST['group'] ?? 'llm'));
                if (!in_array($group, ['llm', 'asr', 'translate'], true)) { $group = 'llm'; }
                $cfg = ai_rw_get_config($pdo, $cid);
                $base = rtrim(trim((string)$cfg[$group . '_base_url']), '/');
                $key  = trim((string)$cfg[$group . '_api_key']);
                $model = trim((string)$cfg[$group . '_model']);
                if ($group === 'translate') {
                    // 翻译组留空时复用 ASR 配置（与客户端读取逻辑一致）
                    if ($base === '') { $base = rtrim(trim((string)$cfg['asr_base_url']), '/'); }
                    if ($key === '') { $key = trim((string)$cfg['asr_api_key']); }
                    if ($model === '') { $model = trim((string)$cfg['asr_model']); }
                }
                if ($base === '' || $key === '' || $model === '') {
                    aijson(['success' => false, 'message' => '配置不完整：接口地址 / API Key / 模型 三项都必须填写']);
                }
                $provider = strtolower(trim((string)($cfg[$group . '_provider'] ?? '')));
                $px_host = trim((string)$cfg[$group . '_proxy']);
                $px_type = strtolower(trim((string)($cfg[$group . '_proxy_type'] ?? '')));
                $px_user = (string)$cfg[$group . '_proxy_user'];
                $px_pass = (string)$cfg[$group . '_proxy_pass'];

                // 请求地址：ASR 用 GET /models 验证地址可达 + Key 有效；其余发一条最小 chat 请求
                if ($group === 'asr') {
                    $url = $base . '/models';
                } elseif ($provider === 'anthropic') {
                    $url = $base . '/v1/messages';
                } else {
                    // OpenAI 兼容（含 gemini 的 OpenAI 兼容端点）
                    $url = $base . '/chat/completions';
                }
                $ch = curl_init($url);
                $headers = [];
                $payload = '';
                if ($group === 'asr') {
                    $headers = ['Authorization: Bearer ' . $key];
                } elseif ($provider === 'anthropic') {
                    $headers = ['Content-Type: application/json', 'x-api-key: ' . $key, 'anthropic-version: 2023-06-01'];
                    $payload = json_encode([
                        'model' => $model,
                        'max_tokens' => 20,
                        'messages' => [['role' => 'user', 'content' => '只回复 ok']],
                    ], JSON_UNESCAPED_UNICODE);
                } else {
                    $headers = ['Content-Type: application/json', 'Authorization: Bearer ' . $key];
                    $payload = json_encode([
                        'model' => $model,
                        'messages' => [['role' => 'user', 'content' => '只回复 ok']],
                        'max_tokens' => 20,
                    ], JSON_UNESCAPED_UNICODE);
                }
                curl_setopt_array($ch, [
                    CURLOPT_POST => $group !== 'asr',
                    CURLOPT_RETURNTRANSFER => true,
                    CURLOPT_TIMEOUT => 60,
                    CURLOPT_CONNECTTIMEOUT => 15,
                    CURLOPT_HTTPHEADER => $headers,
                ]);
                if ($group !== 'asr') { curl_setopt($ch, CURLOPT_POSTFIELDS, $payload); }
                if ($px_host !== '') {
                    $scheme = in_array($px_type, ['socks5', 'socks5h', 'socks'], true) ? 'socks5h://' : 'http://';
                    if ($px_user !== '') { $scheme .= rawurlencode($px_user) . ':' . rawurlencode($px_pass) . '@'; }
                    curl_setopt($ch, CURLOPT_PROXY, $scheme . $px_host);
                }
                $body = curl_exec($ch);
                $errno = curl_errno($ch);
                $err = curl_error($ch);
                $status = (int)curl_getinfo($ch, CURLINFO_HTTP_CODE);
                curl_close($ch);

                if ($body === false || $errno !== 0) {
                    aijson(['success' => false, 'message' => '连接失败（未收到响应）：' . $err . ' — 检查接口地址是否正确、服务器能否访问该地址' . ($px_host !== '' ? '（当前走代理）' : '')]);
                }
                if ($status === 200) {
                    $j = json_decode((string)$body, true);
                    if ($group === 'asr') {
                        $n = isset($j['data']) && is_array($j['data']) ? count($j['data']) : 0;
                        aijson(['success' => true, 'message' => '✅ ASR 接口连通，Key 有效' . ($n > 0 ? '（可见模型 ' . $n . ' 个）' : '')]);
                    }
                    $reply = '';
                    if ($provider === 'anthropic') {
                        $reply = (string)($j['content'][0]['text'] ?? '');
                    } else {
                        $reply = (string)($j['choices'][0]['message']['content'] ?? '');
                    }
                    $reply = trim(mb_substr($reply, 0, 60));
                    aijson(['success' => true, 'message' => '✅ 连接成功，模型「' . $model . '」回复：' . ($reply !== '' ? $reply : '(空)')]);
                }
                // 非 200：把响应体带回，401/403=Key 错，404=路径错，400=模型名或参数错
                $detail = trim((string)$body);
                $detail = preg_replace('/\s+/', ' ', $detail);
                aijson(['success' => false, 'message' => '❌ HTTP ' . $status . '：' . mb_substr($detail, 0, 300)]);
            }

            // ---------- 代理中心列表（网页选代理用） ----------
            case 'proxy_list': {
                $rows = $pdo->prepare("SELECT id,name,proxy_type,host,port,country,city,protocol_config FROM simprint_proxies WHERE company_id = ? ORDER BY id DESC");
                $rows->execute([$cid]);
                $out = [];
                foreach ($rows->fetchAll() as $p) {
                    $pt = strtolower(trim((string)$p['proxy_type']));
                    if (in_array($pt, ['vless','trojan'], true)) {
                        $ehost = ai_rw_proxy_uri($p['proxy_type'], $p['protocol_config'] ?? null);
                    } elseif ($pt === 'ss') {
                        $ehost = null;
                    } else {
                        $ehost = $p['host'] . ':' . (int)$p['port'];
                    }
                    if ($ehost === null || $ehost === '') { continue; }
                    $out[] = [
                        'id' => (int)$p['id'],
                        'name' => $p['name'],
                        'proxy_type' => $p['proxy_type'],
                        'country' => $p['country'] ?? null,
                        'city' => $p['city'] ?? null,
                        'expanded_host' => $ehost,
                    ];
                }
                aijson(['success' => true, 'proxies' => $out]);
            }

            // ---------- 任务列表 ----------
            case 'task_list': {
                $filter = trim((string)($_POST['filter'] ?? ''));  // '' | pending | done | fail
                $keyword = trim((string)($_POST['keyword'] ?? ''));
                $page = max(1, (int)($_POST['page'] ?? 1));
                $page_size = min(100, max(5, (int)($_POST['page_size'] ?? 20)));
                $offset = ($page - 1) * $page_size;

                $where = ["t.company_id = ?"]; $params = [$cid];
                // 洗稿人筛选 tab：-1=我的，0=全部，>0=指定用户
                $filter_uid = (int)($_POST['filter_uid'] ?? 0);
                if ($filter_uid === -1) { $filter_uid = $user_id; } elseif ($filter_uid < 0) { $filter_uid = 0; }
                if ($rw_scope_uid > 0) { $where[] = "t.user_id = ?"; $params[] = $rw_scope_uid; }
                elseif ($filter_uid > 0) { $where[] = "t.user_id = ?"; $params[] = $filter_uid; }
                if ($filter === 'pending') { $where[] = "(t.asr_status IN (0,1) OR t.analyze_status IN (0,1) OR t.rewrite_status IN (0,1))"; }
                elseif ($filter === 'done') { $where[] = "t.asr_status=2 AND t.analyze_status=2 AND t.rewrite_status=2"; }
                elseif ($filter === 'fail') { $where[] = "(t.asr_status=3 OR t.analyze_status=3 OR t.rewrite_status=3)"; }
                if ($keyword !== '') { $where[] = "(t.title LIKE ? OR t.video_id LIKE ?)"; $params[] = '%'.$keyword.'%'; $params[] = '%'.$keyword.'%'; }
                $wsql = implode(' AND ', $where);

                $c = $pdo->prepare("SELECT COUNT(*) FROM ai_rewrite_tasks t WHERE $wsql");
                $c->execute($params); $total = (int)$c->fetchColumn();

                $st = $pdo->prepare("SELECT t.id, t.user_id, t.source, t.video_id, t.platform, t.category, t.title, t.source_url, t.cover_url,
                                            t.play_count, t.video_published_at, t.bgm_name, t.bgm_url, t.asr_status, t.analyze_status, t.rewrite_status, t.retry_count, t.error_msg, t.created_at,
                                            COALESCE(NULLIF(u.real_name, ''), u.username, '') AS rewriter_name
                                     FROM ai_rewrite_tasks t
                                     LEFT JOIN users u ON u.id = t.user_id
                                     WHERE $wsql
                                     ORDER BY t.id DESC LIMIT $page_size OFFSET $offset");
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
                if ($rw_scope_uid > 0 && (int)($task['user_id'] ?? 0) !== $user_id) { aijson(['success' => false, 'message' => '只能查看自己洗稿的任务']); }
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
                        'video_published_at' => (string)($task['video_published_at'] ?? ''),
                        'bgm_name' => (string)($task['bgm_name'] ?? ''), 'bgm_url' => (string)($task['bgm_url'] ?? ''),
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

            // ---------- 🎵 下载视频 BGM（302 到平台原声直链，浏览器直连下载；
            //            47 大陆机连不上 tiktokcdn，但用户本地浏览器可以，且 CDN 带 CORS * 允许前端 fetch 存盘） ----------
            case 'bgm_download': {
                $task_id = (int)($_POST['task_id'] ?? 0);
                $st = $pdo->prepare("SELECT id, bgm_url FROM ai_rewrite_tasks WHERE id = ? AND company_id = ?");
                $st->execute([$task_id, $cid]);
                $trow = $st->fetch(PDO::FETCH_ASSOC);
                if (!$trow || trim((string)$trow['bgm_url']) === '') { aijson(['success' => false, 'message' => '未找到 BGM 原声链接，请先刷新一次播放量']); }
                header('Location: ' . trim((string)$trow['bgm_url']));
                exit;
            }

            // ---------- ✏ 手动修正 BGM 名字（识曲软件认出后人工记录） ----------
            case 'bgm_name_save': {
                $task_id = (int)($_POST['task_id'] ?? 0);
                $name = trim((string)($_POST['bgm_name'] ?? ''));
                if ($name === '') { aijson(['success' => false, 'message' => '名字不能为空']); }
                $st = $pdo->prepare("UPDATE ai_rewrite_tasks SET bgm_name = ? WHERE id = ? AND company_id = ?");
                $st->execute([mb_substr($name, 0, 255), $task_id, $cid]);
                aijson(['success' => true]);
            }

            // ---------- 洗稿人列表（筛选 tab 用） ----------
            case 'rw_user_list': {
                $st = $pdo->prepare("SELECT DISTINCT t.user_id, COALESCE(NULLIF(u.real_name, ''), u.username, '') AS name
                                     FROM ai_rewrite_tasks t LEFT JOIN users u ON u.id = t.user_id
                                     WHERE t.company_id = ? AND t.user_id > 0 ORDER BY name");
                $st->execute([$cid]);
                aijson(['success' => true, 'rows' => $st->fetchAll(PDO::FETCH_ASSOC)]);
            }
            case 'task_delete': {
                $task_id = (int)($_POST['task_id'] ?? 0);
                $task = ai_rw_get_task($pdo, $task_id, $cid);
                if (!$task) { aijson(['success' => false, 'message' => '任务不存在']); }
                if (!$rw_can_manage && (int)$task['user_id'] !== $user_id) { aijson(['success' => false, 'message' => '只能删除自己的洗稿记录']); }
                $pdo->prepare("DELETE FROM ai_rewrite_tasks WHERE id = ?")->execute([$task_id]);
                $pdo->prepare("DELETE FROM viral_points WHERE task_id = ?")->execute([$task_id]);
                $pdo->prepare("DELETE FROM ai_scripts WHERE task_id = ?")->execute([$task_id]);
                aijson(['success' => true, 'message' => '已删除']);
            }

            // ---------- 删除爆款核心（不影响任务记录本身） ----------
            case 'point_delete': {
                $point_id = (int)($_POST['point_id'] ?? 0);
                $st = $pdo->prepare("SELECT vp.user_id, t.user_id AS task_user_id FROM viral_points vp
                                     LEFT JOIN ai_rewrite_tasks t ON t.id = vp.task_id
                                     WHERE vp.id = ? AND vp.company_id = ?");
                $st->execute([$point_id, $cid]);
                $prow = $st->fetch(PDO::FETCH_ASSOC);
                if (!$prow) { aijson(['success' => false, 'message' => '爆款核心不存在']); }
                if (!$rw_can_manage && (int)($prow['user_id'] ?: $prow['task_user_id']) !== $user_id) { aijson(['success' => false, 'message' => '只能删除自己的洗稿记录']); }
                $st = $pdo->prepare("DELETE FROM viral_points WHERE id = ? AND company_id = ?");
                $st->execute([$point_id, $cid]);
                aijson(['success' => true, 'message' => '已删除']);
            }


            // ---------- 🔄 刷新播放量 ----------
            case 'refresh_play_count': {
                $script_id = (int)($_POST['script_id'] ?? 0);
                if ($script_id <= 0) { aijson(['success' => false, 'message' => '参数错误']); }

                $st = $pdo->prepare("SELECT s.task_id, t.platform, t.video_id, t.source_url
                                      FROM ai_scripts s LEFT JOIN ai_rewrite_tasks t ON t.id = s.task_id
                                      WHERE s.id = ? AND s.company_id = ?");
                $st->execute([$script_id, $cid]);
                $row = $st->fetch(PDO::FETCH_ASSOC);
                if (!$row || empty($row['video_id'])) { aijson(['success' => false, 'message' => '未找到视频信息']); }

                $video_id = trim($row['video_id']);
                $source_url = trim($row['source_url'] ?? '');
                // 🔥 从 source_url 推断平台（task.platform 经常为空或不准）
                $platform = strtolower(trim($row['platform'] ?? ''));
                if (empty($platform) && preg_match('#youtube\.com|youtu\.be#i', $source_url)) $platform = 'youtube';
                elseif (empty($platform) && preg_match('#tiktok\.com#i', $source_url)) $platform = 'tiktok';
                elseif (empty($platform) && preg_match('#douyin\.com#i', $source_url)) $platform = 'douyin';
                $platform = strtolower($platform);

                // 计费预检：计费开关开启且余额不足时直接拒绝（关闭时为空操作）
                $pre = ai_bill_precheck($pdo, $cid);
                if ($pre['enabled'] && !$pre['ok']) {
                    aijson(['success' => false, 'message' => $pre['message']]);
                }

                $new_play = 0; $api_success = false; $api_provider = ''; $api_endpoint = 'refresh_play_count';
                $real_vid = ''; $real_vid_yt = ''; $tikwm_last_err = ''; $published_at = '';
                $bgm_name = ''; $bgm_url = '';

                // TikTok / 抖音 → TikWM ?url=完整视频链接（官方 endpoint）
                if ($platform === 'tiktok' || $platform === 'douyin') {
                    // 从 source_url 解析真实 aweme_id（task 里可能存 url_xxx 占位符）
                    $real_vid = $video_id;
                    if (empty($real_vid) || strpos($real_vid, 'url_') === 0 || !preg_match('/^\d+$/', $real_vid)) {
                        if (preg_match('#tiktok\.com/[^/]+/video/(\d+)#i', $source_url, $m)) $real_vid = $m[1];
                        elseif (preg_match('#douyin\.com/video/(\d+)#i', $source_url, $m)) $real_vid = $m[1];
                    }
                    if (empty($real_vid)) {
                        log_api_call($pdo, $cid, 'ai_rewrite', 'tikwm', false, $api_endpoint);
                        aijson(['success' => false, 'message' => '无法解析 TikTok 视频 ID']);
                    }
                    $cs = $pdo->prepare("SELECT tikwm_api_key, tikwm_api_url FROM companies WHERE id = ?");
                    $cs->execute([$cid]); $cfg = $cs->fetch();
                    // TikWM 单条视频 endpoint 要求完整 URL 参数，自建代理不放行，直接走官方
                    $tikwm_base = 'https://api.tikwmapi.com';
                    if ($cfg && !empty($cfg['tikwm_api_url']) && strpos($cfg['tikwm_api_url'], 'tikwm_proxy.php') === false) {
                        $tikwm_base = rtrim($cfg['tikwm_api_url'], '/');
                    }
                    if (!$cfg || empty($cfg['tikwm_api_key'])) {
                        log_api_call($pdo, $cid, 'ai_rewrite', 'tikwm', false, $api_endpoint);
                        aijson(['success' => false, 'message' => 'TikWM API 未配置']);
                    }
                    $api_url = $tikwm_base . '?url=' . urlencode($source_url);
                    $api_provider = 'tikwm';
                    // 成功响应固定为 {"code":0,...}；接口偶发 401（限流抖动），失败重试一次
                    for ($attempt = 0; $attempt < 2 && !$api_success; $attempt++) {
                        if ($attempt > 0) { usleep(400000); }
                        $ch = curl_init($api_url);
                        curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
                        curl_setopt($ch, CURLOPT_TIMEOUT, 20);
                        curl_setopt($ch, CURLOPT_SSL_VERIFYPEER, false);
                        curl_setopt($ch, CURLOPT_HTTPHEADER, ['x-tikwmapi-key: ' . $cfg['tikwm_api_key'], 'Accept: application/json']);
                        $resp = curl_exec($ch); $http_code = curl_getinfo($ch, CURLINFO_HTTP_CODE); $curl_err = curl_error($ch); curl_close($ch);
                        if ($http_code == 200 && $resp) {
                            $data = json_decode($resp, true);
                            if (isset($data['code']) && (int)$data['code'] === 0 && isset($data['data']['play_count'])) {
                                $play = intval($data['data']['play_count']);
                                $new_play = $play;
                                $api_success = ($play > 0);
                                if ($api_success && !empty($data['data']['create_time'])) {
                                    $published_at = date('Y-m-d H:i:s', intval($data['data']['create_time']));
                                }
                                if ($api_success && !empty($data['data']['music_info']['title'])) {
                                    $bgm_name = trim((string)$data['data']['music_info']['title']);
                                    $bgm_url = trim((string)($data['data']['music_info']['play'] ?? ''));
                                }
                            }
                        } else {
                            $tikwm_last_err = 'HTTP ' . $http_code . ($curl_err !== '' ? ' ' . $curl_err : '');
                            if (is_string($resp) && $resp !== '') { $tikwm_last_err .= ' ' . substr($resp, 0, 100); }
                        }
                    }
                    // TikWM 不稳定（偶发 401/超时）→ TikHub 兜底（配置了 key 才启用）
                    if (!$api_success) {
                        $ts = $pdo->prepare("SELECT tikhub_api_url, tikhub_api_key FROM companies WHERE id = ?");
                        $ts->execute([$cid]);
                        $th_cfg = $ts->fetch(PDO::FETCH_ASSOC);
                        $tikhub_key = trim((string)($th_cfg['tikhub_api_key'] ?? ''));
                        if ($tikhub_key !== '' && !empty($real_vid)) {
                            if ($platform === 'tiktok') {
                                $th_url = 'https://api.tikhub.io/api/v1/tiktok/app/v3/fetch_one_video?aweme_id=' . urlencode($real_vid);
                            } else {
                                $th_base = trim((string)($th_cfg['tikhub_api_url'] ?? ''));
                                if ($th_base === '') { $th_base = 'https://api.tikhub.io/api/v1/douyin/app/v3/'; }
                                $th_url = rtrim($th_base, '/') . '/fetch_video_statistics?aweme_ids=' . urlencode($real_vid);
                            }
                            $ch = curl_init($th_url);
                            curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
                            curl_setopt($ch, CURLOPT_TIMEOUT, 25);
                            curl_setopt($ch, CURLOPT_SSL_VERIFYPEER, false);
                            curl_setopt($ch, CURLOPT_HTTPHEADER, ['Authorization: Bearer ' . $tikhub_key, 'Accept: application/json']);
                            $resp = curl_exec($ch); $http_code = curl_getinfo($ch, CURLINFO_HTTP_CODE); curl_close($ch);
                            if ($http_code == 200 && $resp) {
                                $data = json_decode($resp, true);
                                if (isset($data['code']) && (int)$data['code'] === 200) {
                                    $play = 0;
                                    if ($platform === 'tiktok') {
                                        $play = intval($data['data']['aweme_detail']['statistics']['play_count'] ?? 0);
                                        if ($play > 0 && !empty($data['data']['aweme_detail']['create_time'])) {
                                            $published_at = date('Y-m-d H:i:s', intval($data['data']['aweme_detail']['create_time']));
                                        }
                                        if ($play > 0 && !empty($data['data']['aweme_detail']['music']['title'])) {
                                            $bgm_name = trim((string)$data['data']['aweme_detail']['music']['title']);
                                            $bgm_url = trim((string)($data['data']['aweme_detail']['music']['play_url']['url_list'][0] ?? ''));
                                        }
                                    } elseif (!empty($data['data']['statistics_list'][0]['play_count'])) {
                                        $play = intval($data['data']['statistics_list'][0]['play_count']);
                                        if (!empty($data['data']['statistics_list'][0]['create_time'])) {
                                            $published_at = date('Y-m-d H:i:s', intval($data['data']['statistics_list'][0]['create_time']));
                                        }
                                    }
                                    $new_play = $play;
                                    $api_success = ($play > 0);
                                    if ($api_success) { $api_provider = 'tikhub'; }
                                }
                            }
                        }
                    }
                }
                // YouTube（支持逗号分隔多 Key 轮询）
                elseif ($platform === 'youtube') {
                    $cs = $pdo->prepare("SELECT youtube_api_key, youtube_proxy_url FROM companies WHERE id = ?");
                    $cs->execute([$cid]); $cfg = $cs->fetch();
                    $yt_keys = array_filter(array_map('trim', explode(',', $cfg['youtube_api_key'] ?? '')));
                    if (empty($yt_keys)) {
                        log_api_call($pdo, $cid, 'ai_rewrite', 'youtube', false, $api_endpoint);
                        aijson(['success' => false, 'message' => 'YouTube API Key 未配置']);
                    }
                    $proxy = trim($cfg['youtube_proxy_url'] ?? '');
                    $base = $proxy ? rtrim($proxy, '/') : 'https://www.googleapis.com';
                    // 兜底解析真实 video_id
                    $real_vid_yt = $video_id;
                    if (empty($real_vid_yt) || strpos($real_vid_yt, 'url_') === 0) {
                        if (preg_match('#youtube\.com/watch\?v=([a-zA-Z0-9_-]+)#i', $source_url, $m)) $real_vid_yt = $m[1];
                        elseif (preg_match('#youtube\.com/shorts/([a-zA-Z0-9_-]+)#i', $source_url, $m)) $real_vid_yt = $m[1];
                        elseif (preg_match('#youtu\.be/([a-zA-Z0-9_-]+)#i', $source_url, $m)) $real_vid_yt = $m[1];
                    }
                    if (empty($real_vid_yt)) { aijson(['success' => false, 'message' => '无法解析 YouTube 视频 ID']); }
                    $api_provider = 'youtube';
                    // 轮询多 Key，第一个成功即返回
                    foreach ($yt_keys as $idx => $yt_key) {
                        $api_url = $base . '/youtube/v3/videos?part=snippet,statistics&id=' . urlencode($real_vid_yt) . '&key=' . urlencode($yt_key);
                        $ch = curl_init($api_url);
                        curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
                        curl_setopt($ch, CURLOPT_TIMEOUT, 20);
                        curl_setopt($ch, CURLOPT_SSL_VERIFYPEER, false);
                        $resp = curl_exec($ch); $http_code = curl_getinfo($ch, CURLINFO_HTTP_CODE); curl_close($ch);
                        if ($http_code == 200 && $resp) {
                            $data = json_decode($resp, true);
                            if (!empty($data['items']) && is_array($data['items'])) {
                                $new_play = intval($data['items'][0]['statistics']['viewCount'] ?? 0);
                                $api_success = ($new_play > 0);
                                if ($api_success && !empty($data['items'][0]['snippet']['publishedAt'])) {
                                    $ts = strtotime($data['items'][0]['snippet']['publishedAt']);
                                    if ($ts) { $published_at = date('Y-m-d H:i:s', $ts); }
                                }
                                break;
                            }
                        }
                        // 最后一个 key 也没成功
                        if ($idx === count($yt_keys) - 1) {
                            log_api_call($pdo, $cid, 'ai_rewrite', 'youtube', false, $api_endpoint);
                            aijson(['success' => false, 'message' => 'YouTube API 调用失败（已尝试 ' . count($yt_keys) . ' 个 Key）']);
                        }
                    }
                }
                else {
                    aijson(['success' => false, 'message' => '暂不支持的平台: ' . ($platform ?: '未知')]);
                }

                log_api_call($pdo, $cid, 'ai_rewrite', $api_provider, $api_success, $api_endpoint);
                if (!$api_success) {
                    $detail = $tikwm_last_err !== '' ? '（' . $tikwm_last_err . '）' : '';
                    aijson(['success' => false, 'message' => '获取播放量失败' . $detail . '，请稍后重试']);
                }

                // 用解析后的真实 ID 入库，避免 url_xxx 占位符污染
                $final_vid = $video_id;
                if (!empty($real_vid)) $final_vid = $real_vid;
                elseif (!empty($real_vid_yt)) $final_vid = $real_vid_yt;

                // 更新 account_videos：表按天快照、无 (company_id,video_id) 唯一键，
                // 先查后更/插，保证同视频只留一行且播放量只增不减
                $av = $pdo->prepare("SELECT id, play_count FROM account_videos WHERE company_id = ? AND video_id = ? ORDER BY id DESC LIMIT 1");
                $av->execute([$cid, $final_vid]);
                $av_row = $av->fetch(PDO::FETCH_ASSOC);
                if ($av_row) {
                    $av_keep = max((int)$av_row['play_count'], $new_play);
                    $pl_tasks = $pdo->prepare("UPDATE account_videos SET play_count = ?, platform = ? WHERE id = ?");
                    $pl_tasks->execute([$av_keep, $row['platform'], $av_row['id']]);
                } else {
                    // account_type/account_id 仅 NOT NULL 槽位（子查询只取 cur_play）；
                    // benchmark + 0 表示洗稿任务来源，不与真实对标账号混淆
                    $pl_tasks = $pdo->prepare("INSERT INTO account_videos (company_id, account_type, account_id, video_id, play_count, created_at, platform)
                                              VALUES (?, 'benchmark', 0, ?, ?, NOW(), ?)");
                    $pl_tasks->execute([$cid, $final_vid, $new_play, $row['platform']]);
                }

                // 更新 ai_rewrite_tasks 快照（发布时间/BGM 只在取到新值时覆盖，COALESCE 保护旧值）
                if (!empty($row['task_id'])) {
                    $ut = $pdo->prepare("UPDATE ai_rewrite_tasks SET play_count = ?,
                                            video_published_at = COALESCE(?, video_published_at),
                                            bgm_name = COALESCE(NULLIF(?, ''), bgm_name),
                                            bgm_url = COALESCE(NULLIF(?, ''), bgm_url)
                                            WHERE id = ? AND company_id = ?");
                    $ut->execute([$new_play, $published_at !== '' ? $published_at : null, $bgm_name, $bgm_url, $row['task_id'], $cid]);
                }

                // 更新 viral_points 如果有
                $vp_tasks = $pdo->prepare("UPDATE viral_points SET play_count = ? WHERE task_id = ? AND company_id = ?");
                $vp_tasks->execute([$new_play, $row['task_id'], $cid]);

                // 刷新成功后按次扣费（计费开关关闭时 charged=0，仅留流水）
                $bill = ai_bill_charge($pdo, $cid, 'refresh_play_count', ['calls' => 1],
                    ['endpoint' => 'refresh_play_count', 'video_id' => $final_vid, 'play_count' => $new_play]);

                aijson(['success' => true, 'message' => '已刷新', 'play_count' => $new_play, 'current_play_count' => $new_play,
                        'bgm_name' => $bgm_name, 'bgm_url' => $bgm_url,
                        'cost' => $bill['charged'], 'balance_after' => $bill['balance_after']]);
            }
            // ---------- 🔥 合并删除：删文案 + 删爆款核心 ----------
            case 'combined_delete': {
                $script_id = (int)($_POST['script_id'] ?? 0);
                $point_id = (int)($_POST['point_id'] ?? 0);
                // 行级所有权：员工只能删自己的（文案按洗稿人，爆点按爆点/任务归属）
                if (!$rw_can_manage) {
                    if ($script_id > 0) {
                        $st = $pdo->prepare("SELECT user_id FROM ai_scripts WHERE id = ? AND company_id = ?");
                        $st->execute([$script_id, $cid]);
                        $sid_uid = (int)$st->fetchColumn();
                        if ($sid_uid !== $user_id) { aijson(['success' => false, 'message' => '只能删除自己的洗稿记录']); }
                    }
                    if ($point_id > 0) {
                        $st = $pdo->prepare("SELECT COALESCE(vp.user_id, t.user_id) FROM viral_points vp
                                             LEFT JOIN ai_rewrite_tasks t ON t.id = vp.task_id
                                             WHERE vp.id = ? AND vp.company_id = ?");
                        $st->execute([$point_id, $cid]);
                        $pid_uid = (int)$st->fetchColumn();
                        if ($pid_uid !== $user_id) { aijson(['success' => false, 'message' => '只能删除自己的洗稿记录']); }
                    }
                }
                $del_script = 0; $del_point = 0;
                if ($script_id > 0) {
                    $pdo->prepare("DELETE FROM ai_scripts WHERE id = ? AND company_id = ?")->execute([$script_id, $cid]);
                    $del_script = $pdo->query("SELECT ROW_COUNT()")->fetchColumn();
                }
                if ($point_id > 0) {
                    $pdo->prepare("DELETE FROM viral_points WHERE id = ? AND company_id = ?")->execute([$point_id, $cid]);
                    $del_point = $pdo->query("SELECT ROW_COUNT()")->fetchColumn();
                }
                aijson(['success' => true, 'message' => '已删除（文案 + 爆款核心）', 'del_script' => $del_script, 'del_point' => $del_point]);
            }

            // ---------- 编辑爆款核心 ----------
            case 'point_update': {
                $point_id = (int)($_POST['point_id'] ?? 0);
                if ($point_id <= 0) { aijson(['success' => false, 'message' => '缺少 point_id']); }
                $st = $pdo->prepare("SELECT * FROM viral_points WHERE id = ? AND company_id = ?");
                $st->execute([$point_id, $cid]);
                $point = $st->fetch(PDO::FETCH_ASSOC);
                if (!$point) { aijson(['success' => false, 'message' => '爆款核心不存在']); }
                $updates = []; $params = [];
                foreach (['title','work','category','hook','reusable','keywords','summary','emotion'] as $f) {
                    if (isset($_POST[$f])) { $updates[] = "$f = ?"; $params[] = (string)$_POST[$f]; }
                }
                if (!empty($updates)) {
                    $updates[] = "updated_at = NOW()";
                    $params[] = $point_id; $params[] = $cid;
                    $sql = "UPDATE viral_points SET " . implode(',', $updates) . " WHERE id = ? AND company_id = ?";
                    $pdo->prepare($sql)->execute($params);
                }
                aijson(['success' => true, 'message' => '已更新']);
            }

            // ---------- 文案 & 爆款 合并列表 ----------
            case 'combined_list': {
                $keyword = trim((string)($_POST['keyword'] ?? ''));
                $page = max(1, (int)($_POST['page'] ?? 1));
                $page_size = min(100, max(5, (int)($_POST['page_size'] ?? 20)));
                $offset = ($page - 1) * $page_size;

                $where = ["s.company_id = ?"]; $params = [$cid];
                $filter_uid = (int)($_POST['filter_uid'] ?? 0);
                if ($filter_uid === -1) { $filter_uid = $user_id; } elseif ($filter_uid < 0) { $filter_uid = 0; }
                if ($rw_scope_uid > 0) {
                    $where[] = "(s.user_id = ? OR EXISTS(SELECT 1 FROM ai_rewrite_tasks tt WHERE tt.id = s.task_id AND tt.user_id = ?))";
                    $params[] = $rw_scope_uid; $params[] = $rw_scope_uid;
                } elseif ($filter_uid > 0) {
                    $where[] = "(s.user_id = ? OR EXISTS(SELECT 1 FROM ai_rewrite_tasks tt WHERE tt.id = s.task_id AND tt.user_id = ?))";
                    $params[] = $filter_uid; $params[] = $filter_uid;
                }
                if ($keyword !== '') { $where[] = "(s.title LIKE ? OR s.full_text LIKE ? OR s.hook LIKE ? OR vp.hook LIKE ? OR vp.keywords LIKE ? OR vp.summary LIKE ?)"; for ($i=0;$i<6;$i++) $params[] = '%'.$keyword.'%'; }
                $wsql = implode(' AND ', $where);

                $c = $pdo->prepare("SELECT COUNT(*) FROM ai_scripts s LEFT JOIN viral_points vp ON vp.task_id = s.task_id AND vp.company_id = s.company_id WHERE $wsql");
                $c->execute($params); $total = (int)$c->fetchColumn();

                // 🔥 合并查询：以文案为主，LEFT JOIN 爆款核心（同一 task_id 一一对应）
                $heat_src = "COALESCE(NULLIF(vp.play_count, 0), t.play_count, 0)";
                $heat = "(LOG10(($heat_src) + 1) * POW(0.5, DATEDIFF(NOW(), vp.created_at) / 90))";
                $st = $pdo->prepare("SELECT s.id, s.task_id, s.category, s.title, s.hook, s.body, s.cta, s.tags, s.kept_points, s.full_text,
                                            s.score, s.score_json, s.scored_at, s.status, s.created_at, s.user_id,
                                            COALESCE(NULLIF(u.real_name, ''), u.username, '') AS rewriter_name,
                                            vp.id AS point_id, vp.hook AS point_hook, vp.emotion, vp.keywords, vp.reusable, vp.summary,
                                            vp.structure_json, vp.points_json,
                                            vp.play_count AS point_play_count, vp.created_at AS point_created_at,
                                            t.play_count AS task_play_count, t.source_url, t.platform, t.video_published_at AS task_published_at, COALESCE(vp.video_id, t.video_id) AS video_id,
                                            t.bgm_name, t.bgm_url,
                                            ROUND(CASE WHEN vp.id IS NULL THEN NULL ELSE $heat END, 2) AS heat,
                                            COALESCE(v.cur_play, 0) AS current_play_count
                                     FROM ai_scripts s
                                     LEFT JOIN users u ON u.id = s.user_id
                                     LEFT JOIN viral_points vp ON vp.task_id = s.task_id AND vp.company_id = s.company_id
                                     LEFT JOIN ai_rewrite_tasks t ON t.id = s.task_id
                                     LEFT JOIN (SELECT company_id, video_id, MAX(play_count) AS cur_play
                                                FROM account_videos GROUP BY company_id, video_id) v
                                            ON v.company_id = s.company_id AND v.video_id = COALESCE(vp.video_id, t.video_id)
                                     WHERE $wsql ORDER BY s.id DESC LIMIT $page_size OFFSET $offset");
                $st->execute($params);
                $rows = $st->fetchAll(PDO::FETCH_ASSOC);
                foreach ($rows as &$r) {
                    $r['score'] = ($r['score'] === null) ? null : (int)$r['score'];
                    $r['score_data'] = $r['score_json'] ? ai_rw_extract_json((string)$r['score_json']) : null;
                    unset($r['score_json']);
                    $r['structure'] = $r['structure_json'] ? json_decode((string)$r['structure_json'], true) : [];
                    $r['points'] = $r['points_json'] !== null ? explode("\n", (string)$r['points_json']) : [];
                    $r['heat'] = $r['heat'] !== null ? (float)$r['heat'] : null;
                }
                unset($r);
                aijson(['success' => true, 'total' => $total, 'page' => $page, 'page_size' => $page_size, 'rows' => $rows]);
            }

            // ---------- 爆款核心库（单独列表，保留） ----------
            case 'point_list': {
                $keyword = trim((string)($_POST['keyword'] ?? ''));
                $category = trim((string)($_POST['category'] ?? ''));
                $page = max(1, (int)($_POST['page'] ?? 1));
                $page_size = min(100, max(5, (int)($_POST['page_size'] ?? 20)));
                $offset = ($page - 1) * $page_size;

                $where = ["vp.company_id = ?"]; $params = [$cid];
                $filter_uid = (int)($_POST['filter_uid'] ?? 0);
                if ($filter_uid === -1) { $filter_uid = $user_id; } elseif ($filter_uid < 0) { $filter_uid = 0; }
                if ($rw_scope_uid > 0) { $where[] = "(vp.user_id = ? OR t.user_id = ?)"; $params[] = $rw_scope_uid; $params[] = $rw_scope_uid; }
                elseif ($filter_uid > 0) { $where[] = "(vp.user_id = ? OR t.user_id = ?)"; $params[] = $filter_uid; $params[] = $filter_uid; }
                if ($keyword !== '') { $where[] = "(vp.hook LIKE ? OR vp.reusable LIKE ? OR vp.keywords LIKE ? OR vp.summary LIKE ? OR vp.title LIKE ? OR t.title LIKE ? OR vp.work LIKE ?)"; for ($i=0;$i<7;$i++) $params[] = '%'.$keyword.'%'; }
                if ($category !== '') { $where[] = "vp.category = ?"; $params[] = $category; }
                $wsql = implode(' AND ', $where);

                $c = $pdo->prepare("SELECT COUNT(*) FROM viral_points vp LEFT JOIN ai_rewrite_tasks t ON t.id = vp.task_id WHERE $wsql");
                $c->execute($params); $total = (int)$c->fetchColumn();

                // 🔥 回填缺失字段：title/play_count 为空时从 task/采集表补；work 用 title 兜底
                // 热度 = 播放量对数 × 时间衰减（90 天半衰期）
                $heat_src = "CASE WHEN vp.play_count > 0 THEN vp.play_count ELSE COALESCE(v.cur_play, t.play_count, 0) END";
                $heat = "(LOG10(($heat_src) + 1) * POW(0.5, DATEDIFF(NOW(), vp.created_at) / 90))";
                $st = $pdo->prepare("SELECT vp.id, vp.task_id, vp.video_id, vp.platform, vp.category,
                                            COALESCE(NULLIF(vp.work, ''), NULLIF(t.title, ''), '') AS work,
                                            COALESCE(NULLIF(vp.title, ''), NULLIF(t.title, ''), '') AS title,
                                            vp.source_url,
                                            $heat_src AS play_count,
                                            vp.hook, vp.structure_json, vp.points_json,
                                            vp.emotion, vp.keywords, vp.reusable, vp.summary, vp.created_at,
                                            vp.episode,
                                            ROUND($heat, 2) AS heat,
                                            COALESCE(v.cur_play, 0) AS current_play_count,
                                            t.title AS task_title_raw, t.play_count AS task_play_count,
                                            COALESCE(vp.user_id, t.user_id) AS row_user_id,
                                            COALESCE(NULLIF(u.real_name, ''), u.username, '') AS rewriter_name
                                     FROM viral_points vp
                                     LEFT JOIN ai_rewrite_tasks t ON t.id = vp.task_id
                                     LEFT JOIN users u ON u.id = COALESCE(NULLIF(vp.user_id, 0), t.user_id, 0)
                                     LEFT JOIN (SELECT company_id, video_id, MAX(play_count) AS cur_play
                                                FROM account_videos GROUP BY company_id, video_id) v
                                            ON v.company_id = vp.company_id AND v.video_id = vp.video_id
                                     WHERE $wsql ORDER BY $heat DESC, ($heat_src) DESC, vp.id DESC LIMIT $page_size OFFSET $offset");
                $st->execute($params);
                $rows = $st->fetchAll(PDO::FETCH_ASSOC);
                foreach ($rows as &$r) {
                    $r['play_count'] = (int)$r['play_count'];
                    $r['current_play_count'] = (int)$r['current_play_count'];
                    $r['episode'] = (int)$r['episode'];
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

                $where = ["s.company_id = ?"]; $params = [$cid];
                $filter_uid = (int)($_POST['filter_uid'] ?? 0);
                if ($filter_uid === -1) { $filter_uid = $user_id; } elseif ($filter_uid < 0) { $filter_uid = 0; }
                if ($rw_scope_uid > 0) {
                    $where[] = "(s.user_id = ? OR EXISTS(SELECT 1 FROM ai_rewrite_tasks tt WHERE tt.id = s.task_id AND tt.user_id = ?))";
                    $params[] = $rw_scope_uid; $params[] = $rw_scope_uid;
                } elseif ($filter_uid > 0) {
                    $where[] = "(s.user_id = ? OR EXISTS(SELECT 1 FROM ai_rewrite_tasks tt WHERE tt.id = s.task_id AND tt.user_id = ?))";
                    $params[] = $filter_uid; $params[] = $filter_uid;
                }
                if ($keyword !== '') { $where[] = "(s.title LIKE ? OR s.full_text LIKE ?)"; $params[] = '%'.$keyword.'%'; $params[] = '%'.$keyword.'%'; }
                $wsql = implode(' AND ', $where);

                $c = $pdo->prepare("SELECT COUNT(*) FROM ai_scripts s WHERE $wsql");
                $c->execute($params); $total = (int)$c->fetchColumn();

                // 🔥 带出洗稿人（生成文案的用户）与创建时间
                $st = $pdo->prepare("SELECT s.id, s.task_id, s.category, s.title, s.hook, s.body, s.cta, s.tags, s.kept_points, s.full_text,
                                            s.score, s.score_json, s.scored_at, s.status, s.created_at, s.user_id,
                                            COALESCE(NULLIF(u.real_name, ''), u.username, '') AS rewriter_name
                                     FROM ai_scripts s
                                     LEFT JOIN users u ON u.id = s.user_id
                                     WHERE $wsql ORDER BY s.id DESC LIMIT $page_size OFFSET $offset");
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
                if (!$rw_can_manage) { aijson(['success' => false, 'message' => '普通员工只能查看自己洗稿的记录，无打分权限']); }
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
                if (!$rw_can_manage) {
                    $st = $pdo->prepare("SELECT user_id FROM ai_scripts WHERE id = ? AND company_id = ?");
                    $st->execute([$sid, $cid]);
                    $sid_uid = (int)$st->fetchColumn();
                    if ($sid_uid !== $user_id) { aijson(['success' => false, 'message' => '只能删除自己的洗稿记录']); }
                }
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
        .container { width:100%; margin:0 auto; padding:0 12px 80px; }
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
        /* 内层子tab */
        .inner-panel { display:none; } .inner-panel.on { display:block; }
        .subtabs button[data-inner] { padding:8px 18px; font-size:13px; border:none; background:transparent; border-radius:8px; cursor:pointer; color:#666; }
        .subtabs button[data-inner].on { background:#667eea; color:white; }

        /* 弹窗 */
        .modal-mask { display:none; position:fixed; inset:0; background:rgba(0,0,0,.45); z-index:999; }
        .modal-mask.on { display:flex; align-items:flex-start; justify-content:center; padding:90px 16px 40px; overflow:auto; }
        .modal { background:#fff; border-radius:14px; width:min(1040px,96vw); padding:22px; }
        .modal h3 { font-size:16px; margin-bottom:12px; }
        .modal .close { float:right; cursor:pointer; color:#999; font-size:20px; line-height:1; }
        pre.box { background:#f7f8fb; border:1px solid #eee; border-radius:8px; padding:12px; font-size:12.5px; white-space:pre-wrap; word-break:break-word; max-height:340px; overflow:auto; }
        .toast { position:fixed; top:20px; right:20px; background:#333; color:#fff; padding:12px 18px; border-radius:10px; font-size:13px; z-index:2000; opacity:0; transition:opacity .25s; max-width:340px; }
        .toast.on { opacity:1; }
        .row-actions { display:flex; gap:6px; flex-wrap:wrap; }
        @media (max-width:640px){ .grid2 { grid-template-columns:1fr; } }
    .px-combo{position:relative;}
.px-box{display:flex;align-items:center;justify-content:space-between;border:1px solid #d5dbe6;border-radius:6px;padding:8px 10px;cursor:pointer;background:#fff;font-size:14px;}
.px-arrow{color:#8a93a6;}
.px-pop{position:absolute;z-index:50;left:0;right:0;top:calc(100% + 4px);background:#fff;border:1px solid #d5dbe6;border-radius:6px;box-shadow:0 8px 24px rgba(15,30,60,.12);}
.px-search{width:100%;box-sizing:border-box;border:none;border-bottom:1px solid #eef1f6;padding:8px 10px;font-size:13px;outline:none;}
.px-list{max-height:240px;overflow-y:auto;}
.px-item{padding:8px 10px;font-size:13px;cursor:pointer;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}
.px-item:hover{background:#f2f6ff;}
.px-item.active{background:#e8f0ff;color:#1a56db;font-weight:600;}
.px-foot{padding:6px 10px;font-size:12px;color:#8a93a6;border-top:1px solid #eef1f6;}
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
        <button class="on" data-sub="scripts_points" onclick="subTab('scripts_points')">� 文案 & 爆款</button>
        <button data-sub="tasks" onclick="subTab('tasks')">🗂 全部任务</button>
        <?php if ($rw_can_manage): ?><button data-sub="cfg" onclick="subTab('cfg')">⚙️ AI 接口配置</button><?php endif; ?>
    </div>
    <!-- 洗稿人筛选 tab：全部 / 我的 / 按人 -->
    <div class="subtabs" id="personTabs" style="margin-bottom:16px;"></div>
    <div class="subpanel on" id="sub-scripts_points">
        <div class="step">
            <div class="step-head">
                <div class="step-num">�</div>
                <div>
                    <h2>文案 & 爆款核心</h2>
                    <div class="sub">每条文案右侧展示对应的爆款核心（钩子 / 热度 / 关键词），可分别编辑或删除</div>
                </div>
            </div>
            <div class="filters">
                <input type="text" id="cpKeyword" placeholder="搜索标题 / 正文 / 钩子 / 关键词" onkeydown="if(event.key==='Enter')loadCombined(1)">
                <button class="btn" onclick="loadCombined(1)">查询</button>
                <button class="btn green" id="scoreBatchBtn" onclick="scoreSelected()">🎯 打分选中</button>
                <span class="muted" id="scoreTip">已选 0 条</span>
            </div>
            <div style="overflow:auto;">
                <table>
                    <thead><tr>
                        <th style="width:34px;"><input type="checkbox" id="cpSelAll" onchange="toggleAllScripts(this)"></th>
                        <th>文案</th><th>赛道</th><th>洗稿人</th><th>日期</th><th>评分</th>
                        <th>🔗 视频来源</th><th>原播放量</th><th>BGM</th>
                        <th style="min-width:220px;">🔥 爆款核心（钩子 / 热度 / 关键词）</th>
                        <th>操作</th>
                    </tr></thead>
                    <tbody id="cpBody"><tr><td colspan="11" class="empty">加载中…</td></tr></tbody>
                </table>
            </div>
            <div class="pager" id="cpPager"></div>
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
                <thead><tr><th>#</th><th>标题</th><th>平台</th><th>播放量</th><th>转写</th><th>分析</th><th>洗稿</th><th>洗稿人</th><th>操作</th></tr></thead>
                <tbody id="tBody"><tr><td colspan="9" class="empty">加载中…</td></tr></tbody>
            </table>
        </div>
        <div class="pager" id="tPager"></div>
        </div>
    </div>

    <!-- ========== AI 接口配置 ========== -->
    <?php if ($rw_can_manage): ?>
    <div class="subpanel" id="sub-cfg">
        <div class="step">
        <div class="step-head">
            <div class="step-num">⚙️</div>
            <div>
                <h2>AI 接口配置</h2>
                <div class="sub">这里的 ASR / LLM 配置会下发给 BW Browser 客户端使用</div>
            </div>
            <button class="btn sm gray" style="margin-left:auto;" onclick="backToRecords()">← 返回记录</button>
        </div>
        <div class="grid2">
            <div class="field"><label>ASR 接口地址 (Base URL)</label><input type="text" id="cAsrBase" placeholder="https://api.siliconflow.cn/v1"></div>
            <div class="field"><label>ASR 模型</label><input type="text" id="cAsrModel" placeholder="FunAudioLLM/SenseVoiceSmall"></div>
        </div>
        <div class="field"><label>ASR API Key</label><input type="text" id="cAsrKey" placeholder="sk-..."></div>
        <div class="field"><label>ASR 代理线路（代理中心，点击展开搜索选择）</label><div class="px-combo" id="cAsrProxyCombo"></div></div>
        <div class="field">
            <button class="btn" id="asrTestBtn" onclick="llmTest('asr','asrTestBtn','asrTestOut')">🔌 测试 ASR 连接</button>
            <div class="muted" id="asrTestOut" style="margin-top:6px;white-space:pre-wrap;display:none;"></div>
        </div>
        <div class="grid2">
            <div class="field"><label>LLM 接口地址 (Base URL)</label><input type="text" id="cLlmBase" placeholder="https://api.deepseek.com/v1"></div>
            <div class="field"><label>LLM 模型</label><input type="text" id="cLlmModel" placeholder="deepseek-chat"></div>
        </div>
        <div class="field"><label>LLM API Key</label><input type="text" id="cLlmKey" placeholder="sk-..."></div>
        <div class="field"><label>LLM 代理线路（代理中心，点击展开搜索选择）</label><div class="px-combo" id="cLlmProxyCombo"></div></div>
        <div class="field">
            <button class="btn" id="llmTestBtn" onclick="llmTest('llm','llmTestBtn','llmTestOut')">🔌 测试文案模型连接</button>
            <div class="muted" id="llmTestOut" style="margin-top:6px;white-space:pre-wrap;display:none;"></div>
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
        <div class="field">
            <button class="btn" id="trTestBtn" onclick="llmTest('translate','trTestBtn','trTestOut')">🔌 测试翻译连接</button>
            <div class="muted" id="trTestOut" style="margin-top:6px;white-space:pre-wrap;display:none;"></div>
        </div>
        <div class="field"><label>翻译提示词（{目标语言} 为占位符）</label><textarea id="cTranslatePrompt" style="min-height:120px;"></textarea></div>
        <button class="btn green" onclick="saveConfig()">保存配置</button>
        <span class="muted" id="cfgTip" style="margin-left:10px;"></span>
        </div>
    </div>
    </div>
    <?php endif; ?>
</div>

<div class="modal-mask" id="modalMask" onclick="if(event.target===this)closeModal()">
    <div class="modal" id="modalBody"></div>
</div>
<div class="toast" id="toast"></div>

<script>
var CID = <?php echo (int)$view_company_id; ?>;
var RW_CAN_MANAGE = <?php echo ($rw_scope_uid === 0 && $rw_can_manage ? 'true' : 'false'); ?>;
var RW_ME = <?php echo (int)$user_id; ?>;
var PF_UID = 0;
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
            if(!r.rows.length) h = '<tr><td colspan="7" class="empty">还没有生成文案</td></tr>';
            r.rows.forEach(function(s){
                window.__scripts[s.id] = s;
                var ts = (s.created_at||'').replace('T',' ').slice(0,16);
                h += '<tr>'+
                     '<td><input type="checkbox" class="ssel" value="'+s.id+'" onchange="updateScoreTip()"></td>'+
                     '<td><div style="max-width:620px;font-weight:600;">'+esc(s.title||('文案 #'+s.id))+'</div>'+
                         '<div class="muted" style="max-width:620px;max-height:38px;overflow:hidden;">'+esc(String(s.full_text||'').replace(/\n/g,' '))+'</div></td>'+
                     '<td>'+esc(s.category||'-')+'</td>'+
                     '<td>'+esc(s.rewriter_name||'-')+'</td>'+
                     '<td style="white-space:nowrap;">'+(ts?esc(ts):'-')+'</td>'+
                     '<td id="ssc-'+s.id+'">'+((s.score===null||s.score===undefined)?'<span class="badge b-gray">未评</span>':scoreBadge(s.score))+'</td>'+
                     '<td><div class="row-actions">'+
                         (RW_CAN_MANAGE ? '<button class="btn sm" style="background:#7c3aed;" onclick="scoreScript('+s.id+')">🎯 打分</button>' : '')+
                         '<button class="btn sm gray" onclick="toggleScript('+s.id+')">查看</button>'+
                         (RW_CAN_MANAGE || Number(s.user_id)===RW_ME ? '<button class="btn sm" style="background:#ef4444;" onclick="delScript('+s.id+')">删除</button>' : '')+
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
    post('script_delete', { script_id: id }).then(function(r){ toast(r.message||'已删除'); loadCombined(CP_PAGE); }).catch(function(e){ toast(e.message); });
}
function delCombined(script_id, point_id){
    var hasPoint = point_id && point_id > 0;
    var msg = '确定删除此条记录？';
    if(hasPoint) msg = '确定删除此条文案和对应的爆款核心？';
    if(!confirm(msg)) return;
    post('combined_delete', { script_id: script_id, point_id: point_id }).then(function(r){
        toast(r.message||'已删除');
        loadCombined(CP_PAGE);
    }).catch(function(e){ toast(e.message); });
}

/* ---------- 页签切换（懒加载：切到哪个页签首次加载数据） ---------- */
var _combinedLoaded = false;
var CP_PAGE = 1;
function subTab(name){
    document.querySelectorAll('.subtabs button[data-sub]').forEach(function(b){ b.classList.toggle('on', b.dataset.sub===name); });
    document.querySelectorAll('.subpanel').forEach(function(p){ p.classList.remove('on'); });
    document.getElementById('sub-'+name).classList.add('on');
    if (name==='cfg') loadConfig();
    if (name==='scripts_points' && !_combinedLoaded) { loadCombined(1); _combinedLoaded = true; }
    if (name==='tasks') loadTasks(1);
}
function backToRecords(){ subTab('scripts_points'); }

/* ---------- BGM 下载 ---------- */
function saveBlob(blob, name){
    var a = document.createElement('a');
    a.href = URL.createObjectURL(blob); a.download = name;
    document.body.appendChild(a); a.click();
    setTimeout(function(){ URL.revokeObjectURL(a.href); a.remove(); }, 1000);
}
function downloadBgm(taskId){
    toast('开始下载 BGM…');
    fetch('ai_rewrite.php', { method:'POST', credentials:'same-origin',
        headers:{'Content-Type':'application/x-www-form-urlencoded'},
        body: new URLSearchParams({action:'bgm_download', company_id: CID, task_id: taskId}) })
    .then(function(r){
        var ct = r.headers.get('content-type')||'';
        if (ct.indexOf('json') >= 0) return r.json().then(function(j){ throw new Error(j.message||'下载失败'); });
        var cd = r.headers.get('content-disposition')||'';
        var m = /filename\*=UTF-8''([^;]+)/.exec(cd);
        var name = m ? decodeURIComponent(m[1]) : ('bgm_'+taskId+'.mp3');
        return r.blob().then(function(b){ saveBlob(b, name); toast('已下载：'+name); });
    }).catch(function(e){ toast(e.message||'下载失败'); });
}
function bgmCellHtml(s){
    if (!s.bgm_url) return '<span class="muted" style="font-size:11px;">未抓取<br><span style="color:#999;">🔄刷新播放量</span></span>';
    return '<div id="cpbgm-'+s.id+'" style="font-size:12px;">'+
        '<div style="max-width:130px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="'+esc(s.bgm_name||'BGM')+'">'+esc(s.bgm_name||'（未标注）')+
        ' <a href="javascript:void(0)" onclick="editBgmName('+s.task_id+','+s.id+')" style="color:#999;" title="手动修正 BGM 名字">✏</a></div>'+
        '<div style="margin-top:2px;white-space:nowrap;">'+
        '<a href="javascript:void(0)" onclick="downloadBgm('+s.task_id+')" style="color:#667eea;">⬇ 下载</a>'+
        '</div></div>';
}
function editBgmName(taskId, scriptId){
    var s = __combined[scriptId];
    var cur = s ? (s.bgm_name||'') : '';
    var name = prompt('填入正确的 BGM 名字（如：手机QQ识曲后手动记录）', cur);
    if (name === null) return;
    name = String(name).trim();
    if (name === '') return;
    post('bgm_name_save', { task_id: taskId, bgm_name: name }).then(function(r){
        if (!r.success) { toast(r.message||'保存失败'); return; }
        if (__combined[scriptId]) __combined[scriptId].bgm_name = name;
        var cell = document.getElementById('cpbgm-'+scriptId);
        if (cell) cell.outerHTML = bgmCellHtml(__combined[scriptId]);
        toast('BGM 名字已更新');
    }).catch(function(e){ toast(e.message||'保存失败'); });
}

/* ---------- 洗稿人筛选 tab ---------- */
function renderPersonTabs(users){
    var h = '<button class="'+(PF_UID===0?'on':'')+'" onclick="setFilterUid(0)">全部</button>';
    (users||[]).forEach(function(u){
        h += '<button class="'+(PF_UID===Number(u.user_id)?'on':'')+'" onclick="setFilterUid('+Number(u.user_id)+')">'+esc(u.name)+'</button>';
    });
    document.getElementById('personTabs').innerHTML = h;
}
function setFilterUid(uid){
    PF_UID = uid;
    renderPersonTabs(window.__personRows||[]);
    loadCombined(1); loadTasks(1); loadPoints(1); _combinedLoaded = true;
}
post('rw_user_list', {}).then(function(r){ if (r.success) { window.__personRows = r.rows||[]; renderPersonTabs(window.__personRows); } }).catch(function(){});

/* ---------- 合并列表：文案 + 爆款核心 ---------- */
var __combined = {};
function loadCombined(page){
    CP_PAGE = page || 1;
    post('combined_list', { keyword: document.getElementById('cpKeyword').value, filter_uid: PF_UID, page: CP_PAGE, page_size: PAGE_SIZE })
        .then(function(r){
            if(!r.success){ toast(r.message||'加载失败'); return; }
            __combined = {};
            var h = '';
            if(!r.rows.length) h = '<tr><td colspan="11" class="empty">还没有洗稿记录</td></tr>';
            r.rows.forEach(function(s){
                __combined[s.id] = s;
                var ts = (s.created_at||'').replace('T',' ').slice(0,16);
                // 🔥 视频来源链接
                var srcHtml = '-';
                if(s.source_url){
                    var platBadge = '';
                    if(s.platform==='douyin') platBadge='🎶';
                    else if(s.platform==='tiktok') platBadge='🎵';
                    else if(s.platform==='youtube') platBadge='▶️';
                    srcHtml = '<a href="'+esc(s.source_url)+'" target="_blank" style="color:#667eea;font-size:12px;">'+platBadge+' 原视频</a>'+(s.task_published_at?' <span style="font-size:11px;color:#999;">发布 '+(s.task_published_at||'').substring(0,10)+'</span>':'');
                }
                // 爆款核心区
                var pointHtml = '<div style="color:#aaa;">无爆款核心</div>';
                if(s.point_id){
                    var heatHtml = s.heat != null ? '<span class="badge b-gray">🔥 '+s.heat+'</span>' : '';
                    var kwHtml = s.keywords ? '<div style="margin-top:4px;font-size:12px;color:#888;">🏷 '+esc(s.keywords)+'</div>' : '';
                    var hookHtml = s.point_hook ? '<div style="font-size:12px;color:#555;margin-top:4px;">📌 '+esc(s.point_hook.slice(0,80))+(s.point_hook.length>80?'…':'')+'</div>' : '';
                    pointHtml = '<div style="font-size:12px;line-height:1.6;">'+heatHtml+hookHtml+kwHtml+'</div>';
                }
                // 📊 播放量（优先采集最新，回填任务快照）
                var playVal = s.current_play_count > 0 ? s.current_play_count : (s.task_play_count || 0);
                var playHtml = playVal > 0 ? '<span style="font-weight:600;color:#667eea;">'+fmtNum(playVal)+'</span>' : '<span style="color:#aaa;font-size:12px;">0</span>';
                if (s.video_id) {
                    playHtml += '<button id="refreshBtn-'+s.id+'" onclick="refreshPlayCount('+s.id+')" style="border:none;background:transparent;cursor:pointer;font-size:13px;margin-left:6px;color:#667eea;vertical-align:middle;" title="刷新播放量">🔄</button>';
                }
                // 🎵 BGM 列（平台抓到的原声名 + 手动改名 + 下载）
                var bgmHtml = '<span class="muted" style="font-size:11px;">-</span>';
                if (s.task_id) {
                    bgmHtml = bgmCellHtml(s);
                }
                h += '<tr>'+
                     '<td><input type="checkbox" class="ssel" value="'+s.id+'" onchange="updateScoreTip()"></td>'+
                     '<td><div style="max-width:400px;font-weight:600;">'+esc(s.title||('文案 #'+s.id))+'</div>'+
                         '<div class="muted" style="max-width:400px;max-height:38px;overflow:hidden;font-size:12px;">'+esc(String(s.full_text||'').replace(/\n/g,' '))+'</div></td>'+
                     '<td>'+esc(s.category||'-')+'</td>'+
                     '<td>'+esc(s.rewriter_name||'-')+'</td>'+
                     '<td style="white-space:nowrap;font-size:12px;color:#888;">'+(ts?esc(ts.slice(5)):'-')+'</td>'+
                     '<td id="cpsc-'+s.id+'">'+((s.score===null||s.score===undefined)?'<span class="badge b-gray">未评</span>':scoreBadge(s.score))+'</td>'+
                     '<td>'+srcHtml+'</td>'+
                     '<td style="text-align:right;font-size:13px;">'+playHtml+'</td>'+
                     '<td style="font-size:12px;">'+bgmHtml+'</td>'+
                     '<td>'+pointHtml+'</td>'+
                     '<td><div class="row-actions">'+
                         '<button class="btn sm gray" onclick="toggleCombined('+s.id+')">详情</button>'+
                         (s.point_id ? '<button class="btn sm" style="background:#ff9800;" onclick="editPoint('+s.point_id+')">✏️ 改爆款</button>' : '')+
                         '<button class="btn sm" style="background:#7c3aed;" onclick="scoreScript('+s.id+')">🎯 打分</button>'+
                         '<button class="btn sm" style="background:#ef4444;" onclick="delCombined('+s.id+','+(s.point_id?s.point_id:'0')+')">🗑 删除</button>'+
                     '</div></td></tr>';
                h += '<tr id="cpdet-'+s.id+'" style="display:none;"><td></td><td colspan="10"><div id="cpdetbox-'+s.id+'"></div></td></tr>';
            });
            document.getElementById('cpBody').innerHTML = h;
            document.getElementById('cpSelAll').checked = false;
            renderPager('cpPager', r.total, r.page, r.page_size, loadCombined);
            updateScoreTip();
        }).catch(function(e){ toast(e.message); });
}
function toggleCombined(id){
    var row = document.getElementById('cpdet-'+id);
    if(!row) return;
    if(row.style.display !== 'none'){ row.style.display='none'; return; }
    row.style.display='';
    var s = (__combined||{})[id];
    var box = document.getElementById('cpdetbox-'+id);
    if(!box || !s) return;
    var h = '';
    // 文案打分结果
    if(s.score_data) h += scoreHtml(s.score_data);
    h += '<div class="script-label">完整文案</div><div class="script-box">'+esc(s.full_text||'')+'</div>';
    // 爆款核心详情
    if(s.point_id){
        h += '<div class="script-label" style="margin-top:16px;">🔥 爆款核心详情</div>';
        h += '<div style="font-size:13px;line-height:1.8;">';
        if(s.hook) h += '<b>钩子：</b>'+esc(s.hook)+'<br>';
        if(s.emotion) h += '<b>情绪：</b>'+esc(s.emotion)+'<br>';
        if(s.reusable) h += '<b>可复用套路：</b>'+esc(s.reusable)+'<br>';
        if(s.keywords) h += '<b>关键词：</b>'+esc(s.keywords)+'<br>';
        if(s.summary) h += '<b>总结：</b>'+esc(s.summary)+'<br>';
        h += '</div>';
        if(s.structure && s.structure.length) {
            var st = s.structure.map(function(x){ return '· ['+(x.stage||'')+'] '+(x.desc||''); }).join('\n');
            h += '<div class="script-label">结构</div><pre class="box">'+esc(st)+'</pre>';
        }
        if(s.points && s.points.length) h += '<div class="script-label">爆点</div><pre class="box">'+esc(s.points.join('\n'))+'</pre>';
    }
    box.innerHTML = h;
}

function refreshPlayCount(script_id){
    var btn = document.getElementById('refreshBtn-'+script_id);
    if (!btn || btn.disabled) return;
    btn.disabled = true; btn.style.opacity = '0.4';
    btn.textContent = '⏳';
    post('refresh_play_count', { script_id: script_id }).then(function(r){
        btn.disabled = false; btn.style.opacity = '';
        if (!r.success) { btn.textContent = '🔄'; toast(r.message || '刷新失败'); return; }
        toast('已刷新: ' + fmtNum(r.play_count) + (r.cost > 0 ? ' · 扣费 ' + Number(r.cost).toFixed(4) + ' 元' : ''));
        // 更新缓存
        if (__combined[script_id]) {
            __combined[script_id].current_play_count = r.current_play_count;
            __combined[script_id].task_play_count = r.play_count;
            // 刷新可能新抓到 BGM：更新缓存与 BGM 列
            if (r.bgm_url && !__combined[script_id].bgm_url) {
                __combined[script_id].bgm_name = r.bgm_name || '';
                __combined[script_id].bgm_url = r.bgm_url;
                var s2 = __combined[script_id];
                var bgmCell = document.getElementById('cpbgm-'+script_id);
                if (!bgmCell) {
                    var td = btn.closest('tr');
                    if (td) {
                        var cells = td.querySelectorAll('td');
                        var bgmTd = cells[8];
                        if (bgmTd) { bgmTd.innerHTML = bgmCellHtml(s2); }
                    }
                }
            }
        }
        // 只更新这一行的播放量单元格
        var val = r.current_play_count > 0 ? r.current_play_count : (r.play_count || 0);
        var cell = btn.closest('td');
        if (cell) {
            var span = val > 0 ? '<span style="font-weight:600;color:#667eea;">'+fmtNum(val)+'</span>' : '<span style="color:#ccc;">-</span>';
            cell.innerHTML = span + btn.outerHTML;
            btn = cell.querySelector('#refreshBtn-'+script_id);
        }
    }).catch(function(e){
        btn.disabled = false; btn.style.opacity = ''; btn.textContent = '🔄';
        toast('刷新出错');
    });
}
function editPoint(point_id){
    var point = (window.__points||{})[point_id] || (__combined||{})[point_id] || null;
    // 从 combined 数据里找
    if(!point){
        for(var k in __combined){ if(__combined[k].point_id == point_id){ point = __combined[k]; break; } }
    }
    if(!point){ toast('数据已刷新，请重新打开'); return; }
    var h = '<span class="close" onclick="closeModal()">×</span>';
    h += '<h3>✏️ 编辑爆款核心 #'+point_id+'</h3>';
    h += '<div class="field"><label>标题</label><input id="ep_title" value="'+esc(point.title||'')+'"></div>';
    h += '<div class="field"><label>作品</label><input id="ep_work" value="'+esc(point.work||'')+'"></div>';
    h += '<div class="field"><label>赛道</label><input id="ep_category" value="'+esc(point.category||'')+'"></div>';
    h += '<div class="field"><label>钩子</label><textarea id="ep_hook" rows="3">'+esc(point.point_hook||point.hook||'')+'</textarea></div>';
    h += '<div class="field"><label>可复用套路</label><textarea id="ep_reusable" rows="2">'+esc(point.reusable||'')+'</textarea></div>';
    h += '<div class="field"><label>关键词</label><input id="ep_keywords" value="'+esc(point.keywords||'')+'"></div>';
    h += '<div class="field"><label>情绪</label><input id="ep_emotion" value="'+esc(point.emotion||'')+'"></div>';
    h += '<div class="field"><label>总结</label><textarea id="ep_summary" rows="2">'+esc(point.summary||'')+'</textarea></div>';
    h += '<div style="text-align:right;margin-top:16px;"><button class="btn" onclick="savePointUpdate('+point_id+')">保存</button></div>';
    document.getElementById('modalBody').innerHTML = h;
    document.getElementById('modalMask').classList.add('on');
}
function savePointUpdate(point_id){
    post('point_update', {
        point_id: point_id,
        title: document.getElementById('ep_title').value,
        work: document.getElementById('ep_work').value,
        category: document.getElementById('ep_category').value,
        hook: document.getElementById('ep_hook').value,
        reusable: document.getElementById('ep_reusable').value,
        keywords: document.getElementById('ep_keywords').value,
        emotion: document.getElementById('ep_emotion').value,
        summary: document.getElementById('ep_summary').value,
    }).then(function(r){
        if(!r.success){ toast(r.message||'保存失败'); return; }
        closeModal();
        toast('已更新');
        loadCombined(CP_PAGE);
    }).catch(function(e){ toast(e.message); });
}

/* ---------- 配置 ---------- */
var PROXY_POOL = [];
var PX_STATE = {
    cAsrProxyCombo: { query:'', value:'', open:false },
    cLlmProxyCombo: { query:'', value:'', open:false }
};
function esc(s){ return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;'); }
function matchProxy(curHost){
    if (!curHost) return '';
    for (var i=0;i<PROXY_POOL.length;i++){
        if (PROXY_POOL[i].expanded_host === curHost) return String(PROXY_POOL[i].id);
    }
    return '__custom__';
}
function pxBuild(id){
    var box = document.getElementById(id);
    if (!box) return;
    box.innerHTML = '<div class="px-box" onclick="pxToggle(\''+id+'\')"><span class="px-label">加载中…</span><span class="px-arrow">▾</span></div>' +
        '<div class="px-pop" style="display:none;">' +
        '<input class="px-search" placeholder="搜索：名称 / IP / 国家" oninput="pxFilter(\''+id+'\', this.value)" onclick="event.stopPropagation()">' +
        '<div class="px-list"></div></div>';
}
function pxRender(id){
    var st = PX_STATE[id];
    var box = document.getElementById(id);
    if (!st || !box) return;
    var label = '直连（不使用代理）';
    if (st.value === '__custom__') label = '自定义（当前保存值，保留不动）';
    else if (st.value){
        for (var i=0;i<PROXY_POOL.length;i++){
            if (String(PROXY_POOL[i].id) === st.value){
                var p = PROXY_POOL[i];
                label = (p.name||('代理#'+p.id))+' ['+String(p.proxy_type).toUpperCase()+']'+(p.country?(' '+p.country):'');
                break;
            }
        }
    }
    box.querySelector('.px-label').textContent = label;
    var pop = box.querySelector('.px-pop');
    if (!st.open){ pop.style.display = 'none'; return; }
    pop.style.display = '';
    var kw = st.query.trim().toLowerCase();
    var items = [];
    for (var j=0;j<PROXY_POOL.length;j++){
        var q = PROXY_POOL[j];
        if (!q.expanded_host) continue;
        if (kw){
            var hay = (String(q.name)+' '+q.expanded_host+' '+String(q.proxy_type)+' '+String(q.country||'')+' '+String(q.city||'')).toLowerCase();
            if (hay.indexOf(kw) < 0) continue;
        }
        items.push(q);
    }
    var html = '<div class="px-item'+(st.value===''?' active':'')+'" onclick="pxPick(\''+id+'\',\'\')">直连（不使用代理）</div>';
    if (st.value === '__custom__'){
        html += '<div class="px-item active" onclick="pxPick(\''+id+'\',\'__custom__\')">自定义（当前保存值，保留不动）</div>';
    }
    for (var k=0;k<items.length;k++){
        var it = items[k];
        var v2 = String(it.id);
        html += '<div class="px-item'+(st.value===v2?' active':'')+'" onclick="pxPick(\''+id+'\',\''+v2+'\')">'+esc(it.name||('代理#'+it.id))+' ['+String(it.proxy_type).toUpperCase()+']'+(it.country?(' '+it.country):'')+'</div>';
    }
    html += '<div class="px-foot">'+items.length+' 条</div>';
    box.querySelector('.px-list').innerHTML = html;
}
function pxToggle(id){ PX_STATE[id].open = !PX_STATE[id].open; pxRender(id); }
function pxFilter(id, q){ PX_STATE[id].query = q; pxRender(id); }
function pxPick(id, v){ PX_STATE[id].value = v; PX_STATE[id].open = false; pxRender(id); }
function pxSet(id, v){ if (PX_STATE[id]) { PX_STATE[id].value = v; } }
document.addEventListener('click', function(e){
    for (var id in PX_STATE){
        var box = document.getElementById(id);
        if (box && !box.contains(e.target) && PX_STATE[id].open){ PX_STATE[id].open = false; pxRender(id); }
    }
});
function pickVal(id){
    var st = PX_STATE[id];
    return st ? st.value : '__keep__';
}
function pickVal(id){ var v = document.getElementById(id).value; return v === '__custom__' ? '__keep__' : v; }
function loadConfig(){
    post('config_get', {}).then(function(r){
        if (!r.success) { toast(r.message||'失败'); return; }
        var c = r.config;
        document.getElementById('cAsrBase').value = c.asr_base_url||'';
        document.getElementById('cAsrModel').value = c.asr_model||'';
        document.getElementById('cAsrKey').value = c.asr_api_key||'';
        document.getElementById('cLlmBase').value = c.llm_base_url||'';
        document.getElementById('cLlmModel').value = c.llm_model||'';
        document.getElementById('cLlmKey').value = c.llm_api_key||'';
        post('proxy_list', {}).then(function(r){
            PROXY_POOL = (r && r.success && r.proxies) || [];
            pxBuild('cAsrProxyCombo'); pxBuild('cLlmProxyCombo');
            pxSet('cAsrProxyCombo', matchProxy(c.asr_proxy||'')); pxRender('cAsrProxyCombo');
            pxSet('cLlmProxyCombo', matchProxy(c.llm_proxy||'')); pxRender('cLlmProxyCombo');
        }).catch(function(){
            pxBuild('cAsrProxyCombo'); pxBuild('cLlmProxyCombo');
            pxSet('cAsrProxyCombo', matchProxy(c.asr_proxy||'')); pxRender('cAsrProxyCombo');
            pxSet('cLlmProxyCombo', matchProxy(c.llm_proxy||'')); pxRender('cLlmProxyCombo');
        });
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
function saveConfig(){
    post('config_save', {
        asr_base_url: document.getElementById('cAsrBase').value,
        asr_model: document.getElementById('cAsrModel').value,
        asr_api_key: document.getElementById('cAsrKey').value,
        asr_proxy_pick: pickVal('cAsrProxyCombo'),
        llm_base_url: document.getElementById('cLlmBase').value,
        llm_model: document.getElementById('cLlmModel').value,
        llm_api_key: document.getElementById('cLlmKey').value,
        llm_proxy_pick: pickVal('cLlmProxyCombo'),
        analyze_prompt: document.getElementById('cAnalyzePrompt').value,
        rewrite_prompt: document.getElementById('cRewritePrompt').value,
        score_prompt: document.getElementById('cScorePrompt').value,
        translate_model: document.getElementById('cTranslateModel').value,
        translate_base_url: document.getElementById('cTranslateBase').value,
        translate_api_key: document.getElementById('cTranslateKey').value,
        translate_prompt: document.getElementById('cTranslatePrompt').value
    }).then(function(r){ toast(r.message||'已保存'); loadConfig(); }).catch(function(e){ toast(e.message); });
}

/* ---------- 测试模型连接（先保存当前表单再实测，保证所见即所测） ---------- */
function llmTest(group, btnId, outId){
    var btn = document.getElementById(btnId), out = document.getElementById(outId);
    btn.disabled = true;
    out.style.display = 'block';
    out.style.color = '#64748b';
    out.textContent = '正在保存配置并发起测试请求…（约 10–30 秒）';
    post('config_save', {
        asr_base_url: document.getElementById('cAsrBase').value,
        asr_model: document.getElementById('cAsrModel').value,
        asr_api_key: document.getElementById('cAsrKey').value,
        asr_proxy_pick: pickVal('cAsrProxyCombo'),
        llm_base_url: document.getElementById('cLlmBase').value,
        llm_model: document.getElementById('cLlmModel').value,
        llm_api_key: document.getElementById('cLlmKey').value,
        llm_proxy_pick: pickVal('cLlmProxyCombo'),
        analyze_prompt: document.getElementById('cAnalyzePrompt').value,
        rewrite_prompt: document.getElementById('cRewritePrompt').value,
        score_prompt: document.getElementById('cScorePrompt').value,
        translate_model: document.getElementById('cTranslateModel').value,
        translate_base_url: document.getElementById('cTranslateBase').value,
        translate_api_key: document.getElementById('cTranslateKey').value,
        translate_prompt: document.getElementById('cTranslatePrompt').value
    }).then(function(){
        return post('llm_test', { group: group });
    }).then(function(r){
        out.style.color = r.success ? '#16a34a' : '#dc2626';
        out.textContent = r.message || (r.success ? '成功' : '失败');
    }).catch(function(e){
        out.style.color = '#dc2626';
        out.textContent = '失败：' + (e.message || e);
    }).finally(function(){
        btn.disabled = false;
        loadConfig();
    });
}

/* ---------- 爆款核心库 ---------- */
function loadPoints(page){
    P_PAGE = page || 1;
    post('point_list', { keyword: document.getElementById('pKeyword').value, category: document.getElementById('pCategory').value, filter_uid: PF_UID, page: P_PAGE, page_size: PAGE_SIZE })
        .then(function(r){
            if (!r.success) { toast(r.message||'加载失败'); return; }
            var h = '';
            if (!r.rows.length) h = '<tr><td colspan="10" class="empty">还没有沉淀爆款核心</td></tr>';
            window.__points = {};
            r.rows.forEach(function(p){
                window.__points[p.id] = p;
                var work = (p.work||'').split('|')[0];
                h += '<tr>' +
                     '<td>'+p.id+'</td>' +
                     '<td>'+(work?esc(work):'<span class="muted">-</span>')+'</td>' +
                     '<td>'+(p.episode>0?'第'+p.episode+'集':'<span class="muted">-</span>')+'</td>' +
                     '<td><div style="max-width:280px;">'+esc(p.title||'-')+'</div><div class="muted">'+platformBadge(p.platform)+'</div></td>' +
                     '<td>'+esc(p.category||'-')+'</td>' +
                     '<td>'+(p.rewriter_name?esc(p.rewriter_name):'<span class="muted">-</span>')+'</td>' +
                     '<td>'+fmtNum(p.play_count)+'</td>' +
                     '<td>'+(p.heat!=null?esc(p.heat):'-')+'</td>' +
                     '<td><div style="max-width:340px;" class="muted">'+esc(p.hook||'-')+'</div></td>' +
                     '<td><button class="btn sm gray" onclick="showPoint('+p.id+')">详情</button> ' +
                     (RW_CAN_MANAGE || Number(p.row_user_id)===RW_ME ? '<button class="btn sm gray" onclick="delPoint('+p.id+')">删除</button>' : '') + '</td>' +
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
        loadCombined(CP_PAGE);
    }).catch(function(e){ toast(e.message); });
}
function showPoint(id){
    var p = (window.__points||{})[id];
    if (!p) { toast('数据已刷新，请重新打开'); return; }
    var h = '<span class="close" onclick="closeModal()">×</span>';
    h += '<h3>🔥 爆款核心 #'+p.id+'</h3>';
    h += '<div style="font-size:13px;line-height:1.9;"><b>标题：</b>'+esc(p.title||'-')+'<br><b>平台：</b>'+esc(p.platform||'-')+'　<b>赛道：</b>'+esc(p.category||'-')+'　<b>作品：</b>'+esc((p.work||'').split('|').join(' / ')||'-')+(p.episode>0?'　<b>集数：</b>第'+p.episode+'集':'')+'　<b>洗稿人：</b>'+esc(p.rewriter_name||'-')+'　<b>入库时播放：</b>'+fmtNum(p.play_count)+(p.current_play_count>0?'　<b>最新采集播放：</b>'+fmtNum(p.current_play_count):'')+'<br><b>情绪：</b>'+esc(p.emotion||'-')+'<br><b>可复用套路：</b>'+esc(p.reusable||'-')+'<br><b>关键词：</b>'+esc(p.keywords||'-')+'</div>';
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
        filter_uid: PF_UID,
        page: T_PAGE, page_size: PAGE_SIZE
    }).then(function(r){
        if (!r.success) { toast(r.message||'加载失败'); return; }
        var h = '';
        if (!r.rows.length) h = '<tr><td colspan="9" class="empty">没有任务</td></tr>';
        r.rows.forEach(function(t){
            h += '<tr>' +
                 '<td>'+t.id+'</td>' +
                 '<td><div style="max-width:340px;">'+esc(t.title||'(无标题)')+'</div><div class="muted">'+esc(t.source)+' · '+esc(t.video_id)+'</div>' +
                     (t.error_msg? '<div style="color:#b91c1c;font-size:12px;max-width:340px;">⚠ '+esc(t.error_msg)+'</div>' : '') + '</td>' +
                 '<td>'+platformBadge(t.platform)+'</td>' +
                 '<td>'+fmtNum(t.play_count)+'<div>'+(t.bgm_url? '<a href="javascript:void(0)" onclick="downloadBgm('+t.id+')" style="color:#667eea;font-size:12px;" title="'+esc(t.bgm_name||'BGM')+'">🎵 下载BGM</a>' : '<span class="muted" style="font-size:11px;">-</span>')+'</div></td>' +
                 '<td>'+stBadge(t.asr_status)+'</td>' +
                 '<td>'+stBadge(t.analyze_status)+'</td>' +
                 '<td>'+stBadge(t.rewrite_status)+'</td>' +
                 '<td>'+esc(t.rewriter_name||'-')+'</td>' +
                 '<td><div class="row-actions">' +
                     '<button class="btn sm gray" onclick="showTask('+t.id+')">详情</button>' +
                     (RW_CAN_MANAGE || Number(t.user_id)===RW_ME ? '<button class="btn sm" style="background:#ef4444;" onclick="delTask('+t.id+')">删除</button>' : '') +
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
        h += '<div style="font-size:13px;margin-bottom:12px;">'+platformBadge(t.platform)+' 播放 '+fmtNum(t.play_count)+' · <a href="'+esc(t.source_url)+'" target="_blank" style="color:#667eea;">原视频</a>'+(t.video_published_at?' <span style="font-size:11px;color:#999;">发布 '+String(t.video_published_at).substring(0,10)+'</span>':'')+' · 转写 '+stBadge(t.asr_status)+' 分析 '+stBadge(t.analyze_status)+' 洗稿 '+stBadge(t.rewrite_status)+'</div>';
        if (t.bgm_name || t.bgm_url) {
            h += '<div style="font-size:13px;margin-bottom:12px;">🎵 BGM：'+esc(t.bgm_name||'（未标注）')+(t.bgm_url? ' <a href="javascript:void(0)" onclick="downloadBgm('+t.id+')" style="color:#667eea;">⬇ 下载原声</a>' : '')+'</div>';
        }
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

/* ---------- 初始化（三区块同页显示，全部加载） ---------- */
loadCombined(1); _combinedLoaded = true;
loadTasks(1);
if (!RW_CAN_MANAGE) {
    var sb = document.getElementById('scoreBatchBtn');
    if (sb) sb.style.display = 'none';
    var sa = document.getElementById('sSelAll');
    if (sa) sa.parentElement.style.display = 'none';
}
</script>
</body>
</html>