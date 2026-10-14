<?php
/**
 * ai_rewrite_api.php — AI 洗稿本地程序网关
 *
 * 分工边界：
 *   本地程序：下载 → ASR → 爆点分析 → 洗稿 → 打分（全部本地完成，走服务器下发的代理）
 *   本网关  ：鉴权 / 下发密钥与提示词 / 链接去重 / 上报入库 / 用量计费 / 待办清单
 *
 * 鉴权：POST username+password 换取 token（表 ai_client_tokens），后续请求带
 *       X-Auth-Token 头或 token 参数。
 *
 * 全部 action：
 *   login      登录换 token
 *   ping       保活
 *   logout     注销
 *   precheck   余额/计费预检（本地开工前调用）
 *   credentials 领取 AI 密钥 + 代理 + 共享提示词
 *   check      链接查重（命中直接复用，不再调用 API）
 *   todo       待办清单（未跑完的任务）
 *   videos     可见视频列表（自有账号 + 对标账号，供本地勾选）
 *   results    已入库的洗稿成果列表
 *   result     单条成果详情（转写 / 爆点分析 / 洗稿文案 / 打分）
 *   report     上报单步结果 + 用量（入库 + 扣费）
 *   prices     查看当前生效价目
 *
 * 依赖：config.php / ai_rewrite_lib.php / ai_key_lib.php / ai_billing_lib.php
 * 兼容：PHP 7.4
 */

require_once __DIR__ . '/config.php';
require_once __DIR__ . '/ai_rewrite_lib.php';
require_once __DIR__ . '/ai_key_lib.php';
require_once __DIR__ . '/ai_billing_lib.php';

header('Content-Type: application/json; charset=utf-8');
header('Access-Control-Allow-Origin: *');
header('Access-Control-Allow-Methods: GET, POST, OPTIONS');
header('Access-Control-Allow-Headers: Content-Type, X-Auth-Token');
if (($_SERVER['REQUEST_METHOD'] ?? '') === 'OPTIONS') { http_response_code(204); exit; }

// 允许 JSON body 与表单两种入参
$INPUT = $_POST;
if (empty($INPUT)) {
    $raw = file_get_contents('php://input');
    if ($raw !== false && $raw !== '') {
        $j = json_decode($raw, true);
        if (is_array($j)) { $INPUT = $j; }
    }
}
function ai_api_in($k, $default = '') {
    global $INPUT;
    return isset($INPUT[$k]) ? $INPUT[$k] : $default;
}
function ai_api_out($d, $code = 200) {
    http_response_code($code);
    echo json_encode($d, JSON_UNESCAPED_UNICODE);
    exit;
}
function ai_api_fail($msg, $code = 200) {
    ai_api_out(['success' => false, 'message' => $msg], $code);
}
/** 取值并统一成字符串（JSON body 里传对象时自动编码） */
function ai_api_str($k, $default = '') {
    $v = ai_api_in($k, $default);
    if (is_array($v) || is_object($v)) { return json_encode($v, JSON_UNESCAPED_UNICODE); }
    return (string)$v;
}

$action = trim((string)ai_api_in('action', $_GET['action'] ?? ''));
if ($action === '') { ai_api_fail('缺少 action 参数', 400); }

ai_key_ensure_client_tables($pdo);
ai_rw_ensure_tables($pdo);
ai_bill_ensure_tables($pdo);

/**
 * 令牌鉴权。返回 [user_id, company_id, username, is_super_admin, ...] 或 null。
 */
function ai_api_auth($pdo) {
    $token = (string)($_SERVER['HTTP_X_AUTH_TOKEN'] ?? ai_api_in('token', $_GET['token'] ?? ''));
    if ($token === '') { return null; }
    try {
        $st = $pdo->prepare("SELECT t.id AS token_id, t.user_id, t.company_id AS token_company_id,
                u.username, u.real_name, u.role, u.is_super_admin, u.company_id
            FROM `ai_client_tokens` t JOIN `users` u ON u.id = t.user_id
            WHERE t.token = ? AND t.expires_at > NOW() LIMIT 1");
        $st->execute([$token]);
        $row = $st->fetch(PDO::FETCH_ASSOC);
        if (!$row) { return null; }
        $pdo->prepare("UPDATE `ai_client_tokens` SET last_used_at = NOW() WHERE id = ?")->execute([$row['token_id']]);
        return $row;
    } catch (Exception $e) {
        return null;
    }
}

/**
 * 计费归属公司：超管可用 company_id 指定，普通用户固定为自己公司。
 */
function ai_api_company($auth) {
    if ((int)$auth['is_super_admin'] === 1) {
        $c = (int)ai_api_in('company_id', 0);
        return $c > 0 ? $c : (int)$auth['company_id'];
    }
    return (int)$auth['company_id'];
}

// ============================================================
// 无需鉴权的接口
// ============================================================

if ($action === 'login') {
    $username = trim((string)ai_api_in('username'));
    $password = (string)ai_api_in('password');
    $device   = mb_substr(trim((string)ai_api_in('device')), 0, 128);
    if ($username === '' || $password === '') { ai_api_fail('用户名和密码不能为空'); }

    try {
        $st = $pdo->prepare("SELECT * FROM `users` WHERE username = ? LIMIT 1");
        $st->execute([$username]);
        $user = $st->fetch(PDO::FETCH_ASSOC);
    } catch (Exception $e) {
        ai_api_fail('数据库异常，请稍后再试');
    }
    if (!$user) { ai_api_fail('用户名或密码错误'); }

    $ok = false;
    if (!empty($user['password'])) {
        if (function_exists('password_verify') && password_verify($password, $user['password'])) { $ok = true; }
        elseif (md5($password) === $user['password']) { $ok = true; }
        elseif ($password === $user['password']) { $ok = true; }
    }
    if (!$ok) { ai_api_fail('用户名或密码错误'); }

    $token = bin2hex(random_bytes(32));
    $expires = date('Y-m-d H:i:s', strtotime('+30 days'));
    try {
        $pdo->prepare("INSERT INTO `ai_client_tokens` (user_id, company_id, token, device, expires_at)
            VALUES (?, ?, ?, ?, ?)")
            ->execute([(int)$user['id'], (int)$user['company_id'], $token, $device, $expires]);
    } catch (Exception $e) {
        ai_api_fail('令牌写入失败');
    }

    $company_id = (int)$user['company_id'];
    $cfg = ai_key_resolve_config($pdo, $company_id);
    $pre = ai_bill_precheck($pdo, $company_id);

    ai_api_out([
        'success' => true,
        'token'   => $token,
        'expires_at' => $expires,
        'user' => [
            'id' => (int)$user['id'],
            'username' => $user['username'],
            'real_name' => isset($user['real_name']) ? $user['real_name'] : '',
            'role' => isset($user['role']) ? $user['role'] : '',
            'is_super_admin' => (int)(isset($user['is_super_admin']) ? $user['is_super_admin'] : 0),
            'company_id' => $company_id,
        ],
        'config_ready' => ai_key_config_ready($cfg),
        'billing' => $pre,
    ]);
}

// ============================================================
// 需鉴权的接口
// ============================================================

$auth = ai_api_auth($pdo);
if (!$auth) { ai_api_fail('未登录或令牌已失效，请重新登录', 401); }
$company_id = ai_api_company($auth);
$user_id = (int)$auth['user_id'];

switch ($action) {

    case 'ping':
        ai_api_out(['success' => true, 'server_time' => date('Y-m-d H:i:s')]);

    case 'logout': {
        $token = (string)($_SERVER['HTTP_X_AUTH_TOKEN'] ?? ai_api_in('token', ''));
        try { $pdo->prepare("DELETE FROM `ai_client_tokens` WHERE token = ?")->execute([$token]); } catch (Exception $e) {}
        ai_api_out(['success' => true]);
    }

    // ---------- 余额 / 计费预检 ----------
    case 'precheck': {
        $pre = ai_bill_precheck($pdo, $company_id);
        $cfg = ai_key_resolve_config($pdo, $company_id);
        ai_api_out([
            'success' => true,
            'billing' => $pre,
            'config_ready' => ai_key_config_ready($cfg),
        ]);
    }

    // ---------- 领取密钥 + 代理 + 共享提示词 ----------
    case 'credentials': {
        $cfg = ai_key_resolve_config($pdo, $company_id);
        if (!ai_key_config_ready($cfg)) {
            ai_api_fail('服务器尚未配置统一的 AI 密钥，请联系管理员');
        }
        ai_api_out([
            'success' => true,
            'asr' => [
                'provider' => $cfg['asr_provider'],
                'base_url' => $cfg['asr_base_url'],
                'api_key'  => $cfg['asr_api_key'],
                'model'    => $cfg['asr_model'],
                'proxy' => [
                    'host' => $cfg['asr_proxy'],
                    'type' => $cfg['asr_proxy_type'],
                    'user' => $cfg['asr_proxy_user'],
                    'pass' => $cfg['asr_proxy_pass'],
                ],
            ],
            'llm' => [
                'provider' => $cfg['llm_provider'],
                'base_url' => $cfg['llm_base_url'],
                'api_key'  => $cfg['llm_api_key'],
                'model'    => $cfg['llm_model'],
                'proxy' => [
                    'host' => $cfg['llm_proxy'],
                    'type' => $cfg['llm_proxy_type'],
                    'user' => $cfg['llm_proxy_user'],
                    'pass' => $cfg['llm_proxy_pass'],
                ],
            ],
            'rewrite_llm' => ai_rw_rewrite_llm_cfg($cfg),
            'prompts' => [
                'analyze' => $cfg['analyze_prompt'],
                'rewrite' => $cfg['rewrite_prompt'],
                'score'   => $cfg['score_prompt'],
                'translate' => $cfg['translate_prompt'],
            ],
            'translate' => [
                'provider' => 'openai_compatible',
                'base_url' => $cfg['translate_base_url'],
                'api_key'  => $cfg['translate_api_key'],
                'model'    => $cfg['translate_model'],
                'proxy' => [
                    'host' => $cfg['asr_proxy'],
                    'type' => $cfg['asr_proxy_type'],
                    'user' => $cfg['asr_proxy_user'],
                    'pass' => $cfg['asr_proxy_pass'],
                ],
            ],
        ]);
    }

    // ---------- 客户端读取可编辑配置（与后台「AI 接口配置」同一份数据） ----------
    case 'ai_config_get': {
        $cfg = ai_rw_get_config($pdo, $company_id);
        $asr_proxy = [
            'host' => (string)$cfg['asr_proxy'],
            'type' => (string)$cfg['asr_proxy_type'],
            'user' => (string)$cfg['asr_proxy_user'],
            'pass' => (string)$cfg['asr_proxy_pass'],
        ];
        ai_api_out([
            'success' => true,
            'asr' => [
                'provider' => (string)$cfg['asr_provider'],
                'base_url' => (string)$cfg['asr_base_url'],
                'api_key'  => (string)$cfg['asr_api_key'],
                'model'    => (string)$cfg['asr_model'],
                'proxy'    => $asr_proxy,
            ],
            'llm' => [
                'provider' => (string)$cfg['llm_provider'],
                'base_url' => (string)$cfg['llm_base_url'],
                'api_key'  => (string)$cfg['llm_api_key'],
                'model'    => (string)$cfg['llm_model'],
                'proxy' => [
                    'host' => (string)$cfg['llm_proxy'],
                    'type' => (string)$cfg['llm_proxy_type'],
                    'user' => (string)$cfg['llm_proxy_user'],
                    'pass' => (string)$cfg['llm_proxy_pass'],
                ],
            ],
            'translate' => [
                'provider' => 'openai_compatible',
                'base_url' => (string)$cfg['translate_base_url'],
                'api_key'  => (string)$cfg['translate_api_key'],
                'model'    => (string)$cfg['translate_model'],
                // 翻译线路固定复用 ASR 代理
                'proxy'    => $asr_proxy,
            ],
            'rewrite_llm' => [
                'provider' => 'openai_compatible',
                'base_url' => (string)($cfg['rewrite_llm_base_url'] ?? ''),
                'api_key'  => (string)($cfg['rewrite_llm_api_key'] ?? ''),
                'model'    => (string)($cfg['rewrite_llm_model'] ?? ''),
                'proxy' => [
                    'host' => (string)($cfg['rewrite_llm_proxy'] ?? ''),
                    'type' => (string)($cfg['rewrite_llm_proxy_type'] ?? ''),
                    'user' => (string)($cfg['rewrite_llm_proxy_user'] ?? ''),
                    'pass' => (string)($cfg['rewrite_llm_proxy_pass'] ?? ''),
                ],
            ],
        ]);
    }

    // ---------- 客户端保存配置（写同一张 ai_rewrite_config 表，云端后台与本程序实时同步） ----------
    case 'ai_config_save': {
        $c = ai_api_in('config', []);
        if (!is_array($c)) { ai_api_fail('配置格式错误'); }
        $data = [];
        $plain_map = [
            'asr'       => ['asr_provider', 'asr_base_url', 'asr_api_key', 'asr_model'],
            'llm'       => ['llm_provider', 'llm_base_url', 'llm_api_key', 'llm_model'],
            'translate' => ['translate_base_url', 'translate_api_key', 'translate_model'],
        ];
        foreach ($plain_map as $key => $cols) {
            $ep = isset($c[$key]) && is_array($c[$key]) ? $c[$key] : [];
            foreach ($cols as $col) {
                $field = preg_replace('/^(asr|llm|translate)_/', '', $col);
                if (isset($ep[$field]) && is_scalar($ep[$field])) { $data[$col] = (string)$ep[$field]; }
            }
        }
        foreach ([['asr', 'asr_proxy', 'asr_proxy_type', 'asr_proxy_user', 'asr_proxy_pass'],
                  ['llm', 'llm_proxy', 'llm_proxy_type', 'llm_proxy_user', 'llm_proxy_pass']] as $px) {
            $ep = isset($c[$px[0]]) && is_array($c[$px[0]]) ? $c[$px[0]] : [];
            $p  = isset($ep['proxy']) && is_array($ep['proxy']) ? $ep['proxy'] : [];
            foreach (['host' => $px[1], 'type' => $px[2], 'user' => $px[3], 'pass' => $px[4]] as $field => $col) {
                if (isset($p[$field]) && is_scalar($p[$field])) { $data[$col] = (string)$p[$field]; }
            }
        }
        // 洗稿专用模型：组内字段即列名；代理留空则运行时跟随主 LLM
        if (isset($c['rewrite_llm']) && is_array($c['rewrite_llm'])) {
            $ep = $c['rewrite_llm'];
            foreach (['base_url' => 'rewrite_llm_base_url', 'api_key' => 'rewrite_llm_api_key', 'model' => 'rewrite_llm_model'] as $field => $col) {
                if (isset($ep[$field]) && is_scalar($ep[$field])) { $data[$col] = (string)$ep[$field]; }
            }
            $p = isset($ep['proxy']) && is_array($ep['proxy']) ? $ep['proxy'] : [];
            foreach (['host' => 'rewrite_llm_proxy', 'type' => 'rewrite_llm_proxy_type', 'user' => 'rewrite_llm_proxy_user', 'pass' => 'rewrite_llm_proxy_pass'] as $field => $col) {
                if (isset($p[$field]) && is_scalar($p[$field])) { $data[$col] = (string)$p[$field]; }
            }
        }
        if (empty($data)) { ai_api_fail('没有需要保存的配置'); }
        ai_rw_save_config($pdo, $company_id, $data, $user_id);
        ai_api_out(['success' => true]);
    }
    // ---------- 链接查重 ----------
    case 'check': {
        $source_url = trim((string)ai_api_in('source_url'));
        $video_id   = trim((string)ai_api_in('video_id'));
        if ($source_url === '' && $video_id === '') { ai_api_fail('缺少 source_url 或 video_id'); }

        if ($video_id === '') { $video_id = ai_rw_derive_video_id($source_url); }
        $source_key = $source_url !== '' ? ai_rw_norm_url($source_url) : '';

        // 1) 同 video_id 已存在任务 → 返回其当前进度
        $task = null;
        try {
            $st = $pdo->prepare("SELECT * FROM `ai_rewrite_tasks` WHERE company_id = ? AND video_id = ? LIMIT 1");
            $st->execute([$company_id, $video_id]);
            $task = $st->fetch(PDO::FETCH_ASSOC);
        } catch (Exception $e) {}

        if ($task) {
            ai_api_out([
                'success' => true,
                'exists'  => true,
                'reusable' => false,
                'video_id' => $video_id,
                'source_key' => $task['source_key'],
                'task' => [
                    'id' => (int)$task['id'],
                    'asr_status' => (int)$task['asr_status'],
                    'analyze_status' => (int)$task['analyze_status'],
                    'rewrite_status' => (int)$task['rewrite_status'],
                    'asr_text' => (string)$task['asr_text'],
                    'analyze_json' => (string)$task['analyze_json'],
                    'rewrite_text' => (string)$task['rewrite_text'],
                    'done' => ai_rw_task_is_done($task),
                ],
            ]);
        }

        // 2) 同链接历史结果 → 可复用
        $cache = $source_key !== '' ? ai_rw_find_cached_task($pdo, $company_id, $source_key) : null;
        if ($cache) {
            ai_api_out([
                'success' => true,
                'exists'  => false,
                'reusable' => true,
                'video_id' => $video_id,
                'source_key' => $source_key,
                'cache' => [
                    'task_id' => (int)$cache['id'],
                    'asr_status' => (int)$cache['asr_status'],
                    'analyze_status' => (int)$cache['analyze_status'],
                    'rewrite_status' => (int)$cache['rewrite_status'],
                    'asr_text' => (string)$cache['asr_text'],
                    'analyze_json' => (string)$cache['analyze_json'],
                    'rewrite_text' => (string)$cache['rewrite_text'],
                ],
            ]);
        }

        ai_api_out([
            'success' => true,
            'exists'  => false,
            'reusable' => false,
            'video_id' => $video_id,
            'source_key' => $source_key,
        ]);
    }

    // ---------- 待办清单 ----------
    case 'todo': {
        $limit = min(200, max(1, (int)ai_api_in('limit', 20)));
        $only_mine = (int)ai_api_in('only_mine', 0);
        $where = ["company_id = ?", "(asr_status IN (0,1) OR analyze_status IN (0,1) OR rewrite_status IN (0,1))"];
        $params = [$company_id];
        if ($only_mine === 1) { $where[] = "user_id = ?"; $params[] = $user_id; }
        $sql = "SELECT id, user_id, platform, category, title, source_url, video_id,
                       asr_status, analyze_status, rewrite_status, asr_text, analyze_json,
                       play_count, digg_count, comment_count, video_duration, created_at
                FROM `ai_rewrite_tasks`
                WHERE " . implode(' AND ', $where) . "
                ORDER BY id DESC LIMIT {$limit}";
        try {
            $st = $pdo->prepare($sql);
            $st->execute($params);
            $rows = $st->fetchAll(PDO::FETCH_ASSOC);
        } catch (Exception $e) {
            ai_api_fail('查询待办失败');
        }
        foreach ($rows as &$r) {
            $r['id'] = (int)$r['id'];
            $r['asr_status'] = (int)$r['asr_status'];
            $r['analyze_status'] = (int)$r['analyze_status'];
            $r['rewrite_status'] = (int)$r['rewrite_status'];
        }
        unset($r);
        ai_api_out(['success' => true, 'total' => count($rows), 'rows' => $rows]);
    }

    // ---------- 价目 ----------
    case 'prices': {
        $rows = ai_bill_list_prices($pdo, $company_id);
        ai_api_out(['success' => true, 'rows' => $rows]);
    }

    // ---------- 视频数据（供本地程序拉取候选视频） ----------
    case 'videos': {
        $platform  = trim((string)ai_api_in('platform'));
        $source    = trim((string)ai_api_in('source'));   // '' | tiktok | benchmark
        $keyword   = trim((string)ai_api_in('keyword'));
        $min_play  = (int)ai_api_in('min_play', 0);
        $account_id = (int)ai_api_in('account_id', 0);
        $skip_done = (int)ai_api_in('skip_done', 1);      // 默认跳过已洗完的，避免重复分析
        $page      = max(1, (int)ai_api_in('page', 1));
        $page_size = min(100, max(1, (int)ai_api_in('page_size', 20)));
        $offset    = ($page - 1) * $page_size;

        $where = ["v.company_id = ?", "v.play_url IS NOT NULL", "v.play_url <> ''"];
        $params = [$company_id];
        if ($source === 'tiktok' || $source === 'benchmark') { $where[] = "v.account_type = ?"; $params[] = $source; }
        if ($platform !== '' && in_array($platform, ['tiktok', 'youtube', 'douyin'], true)) {
            $where[] = "COALESCE(a.platform, b.platform) = ?"; $params[] = $platform;
        }
        if ($min_play > 0) { $where[] = "v.play_count >= ?"; $params[] = $min_play; }
        if ($account_id > 0) { $where[] = "v.account_id = ?"; $params[] = $account_id; }
        if ($keyword !== '') {
            $where[] = "(v.video_description LIKE ? OR v.video_id LIKE ?)";
            $params[] = '%' . $keyword . '%'; $params[] = '%' . $keyword . '%';
        }
        if ($skip_done === 1) {
            $where[] = "NOT EXISTS (SELECT 1 FROM ai_rewrite_tasks t
                        WHERE t.company_id = v.company_id AND t.video_id = v.video_id AND t.rewrite_status = 2)";
        }
        $wsql = implode(' AND ', $where);

        $base = "FROM account_videos v
                 LEFT JOIN tiktok_accounts a ON v.account_type='tiktok' AND a.id = v.account_id
                 LEFT JOIN benchmark_accounts b ON v.account_type='benchmark' AND b.id = v.account_id
                 WHERE $wsql";
        try {
            $cnt = $pdo->prepare("SELECT COUNT(*) $base");
            $cnt->execute($params);
            $total = (int)$cnt->fetchColumn();

            $sql = "SELECT v.video_id, v.account_type, v.account_id, v.play_count, v.digg_count,
                           v.comment_count, v.video_duration, v.video_description, v.video_tags,
                           v.cover_url, v.create_time,
                           COALESCE(a.platform, b.platform) AS platform,
                           COALESCE(a.pure_username, b.username) AS account_handle,
                           COALESCE(NULLIF(a.nickname,''), a.pure_username, a.account_name,
                                    NULLIF(b.nickname,''), b.username) AS account_name,
                           (SELECT COUNT(*) FROM ai_rewrite_tasks t
                             WHERE t.company_id = v.company_id AND t.video_id = v.video_id) AS has_task,
                           (SELECT t.rewrite_status FROM ai_rewrite_tasks t
                             WHERE t.company_id = v.company_id AND t.video_id = v.video_id LIMIT 1) AS task_rewrite_status
                    $base
                    ORDER BY v.play_count DESC
                    LIMIT $page_size OFFSET $offset";
            $st = $pdo->prepare($sql);
            $st->execute($params);
            $rows = $st->fetchAll(PDO::FETCH_ASSOC);
        } catch (Exception $e) {
            ai_api_fail('查询视频数据失败：' . $e->getMessage());
        }
        foreach ($rows as &$r) {
            $r['play_count']    = (int)$r['play_count'];
            $r['digg_count']    = (int)$r['digg_count'];
            $r['comment_count'] = (int)$r['comment_count'];
            $r['video_duration'] = (int)$r['video_duration'];
            $r['has_task']      = (int)$r['has_task'];
            $r['task_rewrite_status'] = ($r['task_rewrite_status'] === null) ? -1 : (int)$r['task_rewrite_status'];
            $r['title'] = mb_substr((string)$r['video_description'], 0, 80);

            // 稳定页面链接：由 video_id + 平台 + 账号 handle 拼出。
            // 不下发 v.play_url —— 它是带签名的 CDN 直链，有时效，过期后无法下载。
            $r['source_url'] = ai_rw_build_video_url($r['platform'] ?? '', $r['video_id'] ?? '', $r['account_handle'] ?? '');
            unset($r['account_handle']);
        }
        unset($r);
        ai_api_out(['success' => true, 'total' => $total, 'page' => $page, 'page_size' => $page_size, 'rows' => $rows]);
    }

    // ---------- 洗稿结果列表（已入库的成果） ----------
    case 'results': {
        $keyword   = trim((string)ai_api_in('keyword'));
        $status    = trim((string)ai_api_in('status'));   // '' | done | pending
        $only_mine = (int)ai_api_in('only_mine', 0);
        $page      = max(1, (int)ai_api_in('page', 1));
        $page_size = min(100, max(1, (int)ai_api_in('page_size', 20)));
        $offset    = ($page - 1) * $page_size;

        $where = ["t.company_id = ?"]; $params = [$company_id];
        if ($only_mine === 1) { $where[] = "t.user_id = ?"; $params[] = $user_id; }
        if ($status === 'done') { $where[] = "t.rewrite_status = 2"; }
        elseif ($status === 'pending') { $where[] = "t.rewrite_status <> 2"; }
        if ($keyword !== '') {
            $where[] = "(t.title LIKE ? OR t.video_id LIKE ? OR t.source_url LIKE ?)";
            $params[] = '%' . $keyword . '%'; $params[] = '%' . $keyword . '%'; $params[] = '%' . $keyword . '%';
        }
        $wsql = implode(' AND ', $where);
        try {
            $cnt = $pdo->prepare("SELECT COUNT(*) FROM ai_rewrite_tasks t WHERE $wsql");
            $cnt->execute($params);
            $total = (int)$cnt->fetchColumn();

            $sql = "SELECT t.id, t.video_id, t.platform, t.title, t.source_url, t.play_count,
                           t.asr_status, t.analyze_status, t.rewrite_status, t.error_msg, t.created_at,
                           (SELECT s.id FROM ai_scripts s WHERE s.task_id = t.id ORDER BY s.id DESC LIMIT 1) AS script_id,
                           (SELECT s.score FROM ai_scripts s WHERE s.task_id = t.id ORDER BY s.id DESC LIMIT 1) AS score
                    FROM ai_rewrite_tasks t
                    WHERE $wsql
                    ORDER BY t.id DESC
                    LIMIT $page_size OFFSET $offset";
            $st = $pdo->prepare($sql);
            $st->execute($params);
            $rows = $st->fetchAll(PDO::FETCH_ASSOC);
        } catch (Exception $e) {
            ai_api_fail('查询结果失败：' . $e->getMessage());
        }
        foreach ($rows as &$r) {
            $r['id'] = (int)$r['id'];
            $r['play_count'] = (int)$r['play_count'];
            $r['asr_status'] = (int)$r['asr_status'];
            $r['analyze_status'] = (int)$r['analyze_status'];
            $r['rewrite_status'] = (int)$r['rewrite_status'];
            $r['script_id'] = ($r['script_id'] === null) ? 0 : (int)$r['script_id'];
            $r['score'] = ($r['score'] === null) ? null : (int)$r['score'];
        }
        unset($r);
        ai_api_out(['success' => true, 'total' => $total, 'page' => $page, 'page_size' => $page_size, 'rows' => $rows]);
    }

    // ---------- 单条结果详情（转写 + 爆点分析 + 洗稿文案 + 打分） ----------
    case 'result': {
        $task_id = (int)ai_api_in('task_id', 0);
        if ($task_id <= 0) { ai_api_fail('缺少 task_id'); }
        $task = ai_rw_get_task($pdo, $task_id, $company_id);
        if (!$task) { ai_api_fail('结果不存在或无权查看'); }

        $script = null;
        try {
            $st = $pdo->prepare("SELECT * FROM ai_scripts WHERE task_id = ? ORDER BY id DESC LIMIT 1");
            $st->execute([$task_id]);
            $script = $st->fetch(PDO::FETCH_ASSOC);
        } catch (Exception $e) {}

        $analyze = null;
        if (!empty($task['analyze_json'])) { $analyze = json_decode((string)$task['analyze_json'], true); }
        $rewrite = null;
        if (!empty($task['rewrite_json'])) { $rewrite = json_decode((string)$task['rewrite_json'], true); }
        $score = null;
        if ($script && !empty($script['score_json'])) { $score = json_decode((string)$script['score_json'], true); }

        ai_api_out([
            'success' => true,
            'task' => [
                'id' => (int)$task['id'],
                'video_id' => (string)$task['video_id'],
                'platform' => (string)$task['platform'],
                'title' => (string)$task['title'],
                'source_url' => (string)$task['source_url'],
                'cover_url' => (string)$task['cover_url'],
                'play_count' => (int)$task['play_count'],
                'digg_count' => (int)$task['digg_count'],
                'comment_count' => (int)$task['comment_count'],
                'video_duration' => (int)$task['video_duration'],
                'asr_status' => (int)$task['asr_status'],
                'analyze_status' => (int)$task['analyze_status'],
                'rewrite_status' => (int)$task['rewrite_status'],
                'error_msg' => (string)$task['error_msg'],
                'created_at' => (string)$task['created_at'],
                'transcript' => (string)$task['asr_text'],
                'analyze' => $analyze,
                'rewrite' => $rewrite,
            ],
            'script' => $script ? [
                'id' => (int)$script['id'],
                'title' => (string)$script['title'],
                'hook' => (string)$script['hook'],
                'body' => (string)$script['body'],
                'cta' => (string)$script['cta'],
                'tags' => (string)$script['tags'],
                'kept_points' => (string)$script['kept_points'],
                'full_text' => (string)$script['full_text'],
                'score' => ($script['score'] === null) ? null : (int)$script['score'],
                'score_json' => $score,
            ] : null,
        ]);
    }

    // ---------- 爆款网感参考（洗稿前检索爆点库：同作品同集 → 作品记忆 → 同赛道 → 跨赛道 → 兜底） ----------
    case 'points_sample': {
        $work     = trim((string)ai_api_in('work'));
        $episode  = (int)ai_api_in('episode', 0);
        $category = trim((string)ai_api_in('category'));
        $emotion  = trim((string)ai_api_in('emotion'));
        $kw_raw   = trim((string)ai_api_in('keywords'));
        $kws = array_values(array_filter(array_map('trim', explode(',', $kw_raw)), function ($k) { return $k !== ''; }));
        $kws = array_slice($kws, 0, 5);

        $empty = ['work' => $work, 'work_summary' => null, 'work_records' => [], 'same_work' => [], 'same_track' => [], 'cross_track' => [], 'fallback' => []];
        try {
            ai_rw_ensure_tables($pdo);
            $heat = "(LOG10(vp.play_count + 1) * POW(0.5, DATEDIFF(NOW(), vp.created_at) / 90))";
            $cols = "vp.id, vp.task_id, vp.video_id, vp.category, vp.work, vp.episode, vp.title, vp.play_count,
                     vp.hook, vp.structure_json, vp.points_json, vp.emotion, vp.keywords";
            // 作品匹配：库里存 "规范名|别名1|别名2"，参数命中任一部分，或参数包含库里的规范名
            $work_match = "(vp.work LIKE CONCAT('%', ?, '%') OR LOCATE(SUBSTRING_INDEX(vp.work, '|', 1), ?) > 0)";

            $slim = function ($rows) {
                $out = [];
                foreach ($rows as $r) {
                    $structure = json_decode((string)($r['structure_json'] ?? ''), true);
                    $points = preg_split('/\n/', (string)($r['points_json'] ?? '')) ?: [];
                    $stages = [];
                    if (is_array($structure)) {
                        foreach (array_slice($structure, 0, 3) as $s) {
                            $line = is_array($s) ? trim(($s['stage'] ?? '') . '：' . ($s['desc'] ?? '')) : trim((string)$s);
                            if ($line !== '' && $line !== '：') { $stages[] = mb_substr($line, 0, 100); }
                        }
                    }
                    $pts = [];
                    foreach (array_slice($points, 0, 3) as $p) {
                        $p = trim((string)$p);
                        if ($p !== '') { $pts[] = mb_substr($p, 0, 120); }
                    }
                    $out[] = [
                        'id' => (int)$r['id'],
                        'category' => (string)$r['category'],
                        'work' => (string)$r['work'],
                        'episode' => (int)($r['episode'] ?? 0),
                        'title' => mb_substr((string)$r['title'], 0, 60),
                        'play_count' => (int)$r['play_count'],
                        'hook' => mb_substr((string)$r['hook'], 0, 160),
                        'stages' => $stages,
                        'points' => $pts,
                        'emotion' => (string)$r['emotion'],
                    ];
                }
                return $out;
            };

            $used = [0];

            // 层⓪⁺ 同作品同集：本集此前处理过的视频与数据（动漫/影视解说的集数维度记录）
            $work_records = [];
            if ($work !== '' && $episode > 0) {
                try {
                    $st = $pdo->prepare("SELECT task_id, video_id, source_url, title, play_count, created_at
                        FROM anime_work_records
                        WHERE company_id = ? AND episode = ? AND work_title <> ''
                          AND (work_title LIKE CONCAT('%', ?, '%') OR LOCATE(SUBSTRING_INDEX(work_title, '|', 1), ?) > 0)
                        ORDER BY id DESC LIMIT 5");
                    $st->execute([$company_id, $episode, $work, $work]);
                    $work_records = $st->fetchAll(PDO::FETCH_ASSOC);
                } catch (Exception $e) { $work_records = []; }
            }

            // 层⓪ 作品记忆：热度前 6 + 最新入库 2（时间衰减不该压住最新经验）
            $same_work = [];
            $work_count = 0;
            $used_hooks = [];
            if ($work !== '') {
                $st = $pdo->prepare("SELECT $cols, $heat AS heat FROM viral_points vp
                    WHERE vp.company_id = ? AND vp.work <> '' AND $work_match AND vp.id NOT IN (" . implode(',', $used) . ")
                    ORDER BY heat DESC LIMIT 6");
                $st->execute([$company_id, $work, $work]);
                $same_work = $st->fetchAll(PDO::FETCH_ASSOC);
                foreach ($same_work as $r) { $used[] = (int)$r['id']; }

                $st = $pdo->prepare("SELECT $cols, $heat AS heat FROM viral_points vp
                    WHERE vp.company_id = ? AND vp.work <> '' AND $work_match AND vp.id NOT IN (" . implode(',', $used) . ")
                    ORDER BY vp.id DESC LIMIT 2");
                $st->execute([$company_id, $work, $work]);
                $latest_work = $st->fetchAll(PDO::FETCH_ASSOC);
                foreach ($latest_work as $r) { $used[] = (int)$r['id']; }
                $same_work = array_merge($same_work, $latest_work);

                $st = $pdo->prepare("SELECT COUNT(*) FROM viral_points vp
                    WHERE vp.company_id = ? AND vp.work <> '' AND $work_match");
                $st->execute([$company_id, $work, $work]);
                $work_count = (int)$st->fetchColumn();

                $st = $pdo->prepare("SELECT s.hook FROM ai_scripts s
                    JOIN viral_points vp ON vp.task_id = s.task_id
                    WHERE s.company_id = ? AND vp.work <> '' AND $work_match AND s.hook <> ''
                    ORDER BY s.id DESC LIMIT 8");
                $st->execute([$company_id, $work, $work]);
                foreach ($st->fetchAll(PDO::FETCH_COLUMN) as $h) { $used_hooks[] = mb_substr((string)$h, 0, 80); }
            }

            // 层① 同赛道（题材钩子）
            $same_track = [];
            if ($category !== '') {
                $st = $pdo->prepare("SELECT $cols, $heat AS heat FROM viral_points vp
                    WHERE vp.company_id = ? AND vp.category = ? AND vp.id NOT IN (" . implode(',', $used) . ")
                    ORDER BY heat DESC LIMIT 12");
                $st->execute([$company_id, $category]);
                $same_track = $st->fetchAll(PDO::FETCH_ASSOC);
                foreach ($same_track as $r) { $used[] = (int)$r['id']; }
            }

            // 层② 跨赛道（情绪/关键词命中的通用网感）
            $cross_track = [];
            $or = [];
            $p2 = [$company_id, $category];
            if ($emotion !== '') { $or[] = "vp.emotion = ?"; $p2[] = $emotion; }
            foreach ($kws as $k) {
                $or[] = "(vp.keywords LIKE ? OR vp.hook LIKE ? OR vp.points_json LIKE ?)";
                $like = '%' . $k . '%';
                array_push($p2, $like, $like, $like);
            }
            if ($or) {
                $st = $pdo->prepare("SELECT $cols, $heat AS heat FROM viral_points vp
                    WHERE vp.company_id = ? AND vp.category <> ? AND vp.category <> '' AND (" . implode(' OR ', $or) . ")
                      AND vp.id NOT IN (" . implode(',', $used) . ")
                    ORDER BY heat DESC LIMIT 6");
                $st->execute($p2);
                $cross_track = $st->fetchAll(PDO::FETCH_ASSOC);
                foreach ($cross_track as $r) { $used[] = (int)$r['id']; }
            }

            // 层③ 兜底（全库最高热）
            $st = $pdo->prepare("SELECT $cols, $heat AS heat FROM viral_points vp
                WHERE vp.company_id = ? AND vp.id NOT IN (" . implode(',', $used) . ")
                ORDER BY heat DESC LIMIT 6");
            $st->execute([$company_id]);
            $fallback = $st->fetchAll(PDO::FETCH_ASSOC);

            ai_api_out([
                'success' => true,
                'work' => $work,
                'work_summary' => $work !== '' ? [
                    'work' => $work,
                    'analyzed_count' => $work_count,
                    'used_hooks' => $used_hooks,
                ] : null,
                'work_records' => $work_records,
                'same_work' => $slim($same_work),
                'same_track' => $slim($same_track),
                'cross_track' => $slim($cross_track),
                'fallback' => $slim($fallback),
            ]);
        } catch (Exception $e) {
            // 检索失败不阻塞洗稿主流程：返回空参考，客户端按未注入处理
            ai_api_out($empty);
        }
    }

    // ---------- 上报单步结果 + 用量（入库 + 扣费） ----------
    case 'report': {
        $step = trim((string)ai_api_in('step'));
        if (!in_array($step, ['asr', 'analyze', 'rewrite', 'score'], true)) {
            ai_api_fail('step 参数不合法（asr/analyze/rewrite/score）');
        }

        $source_url = trim((string)ai_api_in('source_url'));
        $video_id   = trim((string)ai_api_in('video_id'));
        $task_id    = (int)ai_api_in('task_id', 0);

        // 定位或创建任务
        $task = null;
        if ($task_id > 0) { $task = ai_rw_get_task($pdo, $task_id, $company_id); }
        if (!$task && $video_id !== '') {
            try {
                $st = $pdo->prepare("SELECT * FROM `ai_rewrite_tasks` WHERE company_id = ? AND video_id = ? LIMIT 1");
                $st->execute([$company_id, $video_id]);
                $task = $st->fetch(PDO::FETCH_ASSOC);
            } catch (Exception $e) {}
        }
        if (!$task) {
            if ($source_url === '' && $video_id === '') { ai_api_fail('缺少 source_url / video_id / task_id'); }
            $create = ai_rw_create_task($pdo, $company_id, $user_id, [
                'source' => (string)ai_api_in('source', 'local'),
                'video_id' => $video_id,
                'source_url' => $source_url,
                'account_id' => (int)ai_api_in('account_id', 0),
                'account_type' => (string)ai_api_in('account_type', ''),
                'platform' => (string)ai_api_in('platform', ''),
                'category' => (string)ai_api_in('category', ''),
                'title' => (string)ai_api_in('title', ''),
                'cover_url' => (string)ai_api_in('cover_url', ''),
                'play_count' => (int)ai_api_in('play_count', 0),
                'digg_count' => (int)ai_api_in('digg_count', 0),
                'comment_count' => (int)ai_api_in('comment_count', 0),
                'video_duration' => (int)ai_api_in('video_duration', 0),
            ]);
            if (!empty($create['status']) && $create['status'] === 'error') {
                ai_api_fail((string)$create['message']);
            }
            $task = ai_rw_get_task($pdo, (int)$create['task_id'], $company_id);
        }
        if (!$task) { ai_api_fail('任务创建/定位失败'); }

        $usage = json_decode((string)ai_api_in('usage', ''), true);
        if (!is_array($usage)) { $usage = []; }
        $raw_usage = ai_api_str('raw_usage');

        $charged = false;
        $updates = [];
        $result = ['task_id' => (int)$task['id'], 'step' => $step];

        if ($step === 'asr') {
            $text = trim((string)ai_api_in('asr_text'));
            if ($text === '') { ai_api_fail('asr_text 不能为空'); }
            if ((int)$task['asr_status'] !== 2) {
                $updates['asr_text'] = $text;
                $updates['asr_status'] = 2;
                $updates['asr_manual'] = (int)ai_api_in('asr_manual', 0);
                $updates['error_msg'] = '';
                $updates['retry_count'] = 0;
                $charged = true;
            }
        } elseif ($step === 'analyze') {
            $aj = ai_api_str('analyze_json');
            $data = ai_rw_extract_json($aj);
            if ($data === null) { ai_api_fail('analyze_json 不是合法 JSON'); }
            if ((int)$task['analyze_status'] !== 2) {
                $cat = isset($data['category']) ? mb_substr((string)$data['category'], 0, 50) : '';
                $updates['analyze_json'] = json_encode($data, JSON_UNESCAPED_UNICODE);
                $updates['analyze_status'] = 2;
                if ($cat !== '') { $updates['category'] = $cat; }
                $updates['retry_count'] = 0;
                $charged = true;
                // 播放量回写：补拉到的实时数据（live_play_count）或请求携带的列表数据（play_count），
                // 取更大值更新任务表——爆点库与作品集数归档都读任务表的 play_count
                $live_pc = (int)ai_api_in('live_play_count', 0);
                $req_pc = (int)ai_api_in('play_count', 0);
                $best_pc = max($live_pc, $req_pc);
                if ($best_pc > (int)$task['play_count']) { $updates['play_count'] = $best_pc; }
            }
        } elseif ($step === 'rewrite') {
            $rj = ai_api_str('rewrite_json');
            $data = ai_rw_extract_json($rj);
            if ($data === null) { ai_api_fail('rewrite_json 不是合法 JSON'); }
            if ((int)$task['rewrite_status'] !== 2) {
                $updates['rewrite_json'] = json_encode($data, JSON_UNESCAPED_UNICODE);
                $updates['rewrite_text'] = ai_rw_compose_script_text($data);
                $updates['rewrite_status'] = 2;
                $updates['retry_count'] = 0;
                $charged = true;
            }
        } elseif ($step === 'score') {
            $sj = ai_api_str('score_json');
            $data = ai_rw_extract_json($sj);
            if ($data === null) { ai_api_fail('score_json 不是合法 JSON'); }
            $charged = true; // 打分每次都算一次用量
        }

        // 先写业务数据
        if (!empty($updates)) { ai_rw_task_update($pdo, (int)$task['id'], $updates); }
        $task = ai_rw_get_task($pdo, (int)$task['id'], $company_id);

        // analyze → 沉淀爆款核心
        if ($step === 'analyze' && $charged) {
            $analysis = ai_rw_extract_json((string)$task['analyze_json']);
            if (is_array($analysis)) { ai_rw_upsert_viral_point($pdo, $task, $analysis); }

            // 同步归档「作品 + 集数」维度（动漫/影视解说：同片同集下次可查可参考）
            try {
                $w_title = isset($analysis['work']) ? trim((string)$analysis['work']) : '';
                $w_ep = isset($analysis['episode']) ? (int)$analysis['episode'] : 0;
                if (is_array($analysis) && $w_title !== '' && $w_ep > 0) {
                    $st = $pdo->prepare("INSERT INTO anime_work_records
                        (company_id, work_title, episode, task_id, video_id, source_url, title, play_count)
                        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                        ON DUPLICATE KEY UPDATE work_title = VALUES(work_title), episode = VALUES(episode), play_count = VALUES(play_count)");
                    $st->execute([
                        $company_id,
                        mb_substr($w_title, 0, 255),
                        $w_ep,
                        (int)$task['id'],
                        (string)$task['video_id'],
                        (string)$task['source_url'],
                        mb_substr((string)$task['title'], 0, 255),
                        (int)$task['play_count'],
                    ]);
                }
            } catch (Exception $e) { /* 归档失败不影响主流程 */ }
        }
        // rewrite → 文案入库
        if ($step === 'rewrite' && $charged) {
            $data = ai_rw_extract_json((string)$task['rewrite_json']);
            if (is_array($data)) {
                $result['script_id'] = ai_rw_save_script($pdo, $task, $data, ai_rw_get_point_id_by_task($pdo, (int)$task['id']));
            }
        }
        // score → 回写打分
        if ($step === 'score') {
            $script_id = (int)ai_api_in('script_id', 0);
            if ($script_id <= 0) {
                try {
                    $st = $pdo->prepare("SELECT id FROM `ai_scripts` WHERE task_id = ? ORDER BY id DESC LIMIT 1");
                    $st->execute([(int)$task['id']]);
                    $script_id = (int)$st->fetchColumn();
                } catch (Exception $e) { $script_id = 0; }
            }
            if ($script_id <= 0) { ai_api_fail('找不到可打分的文案（请先上报 rewrite）'); }
            $data = ai_rw_extract_json(ai_api_str('score_json'));
            ai_rw_save_score($pdo, $script_id, is_array($data) ? $data : []);
            $result['script_id'] = $script_id;
            $result['score'] = isset($data['score']) ? (int)$data['score'] : 0;
        }

        // 计费（幂等：仅在实际产生新结果时扣费；余额不足不阻断，上限为余额）
        $bill = ['charged' => 0.0, 'balance_after' => 0.0, 'cost' => 0.0, 'mode' => '', 'qty' => 0.0, 'blocked' => 0];
        if ($charged) {
            $bill = ai_bill_charge($pdo, $company_id, $step, $usage, [
                'endpoint' => 'local:' . $step,
                'raw_usage' => $raw_usage,
                'task_id' => (int)$task['id'],
                'video_id' => (string)$task['video_id'],
            ]);
        }

        $result['charged'] = (float)$bill['charged'];
        $result['cost'] = (float)$bill['cost'];
        $result['balance_after'] = (float)$bill['balance_after'];
        $result['billing_mode'] = (string)$bill['mode'];
        $result['usage_qty'] = (float)$bill['qty'];
        $result['blocked'] = (int)$bill['blocked'];
        $result['skipped_billing'] = !$charged;
        $result['task'] = [
            'id' => (int)$task['id'],
            'asr_status' => (int)$task['asr_status'],
            'analyze_status' => (int)$task['analyze_status'],
            'rewrite_status' => (int)$task['rewrite_status'],
            'done' => ai_rw_task_is_done($task),
        ];
        ai_api_out(['success' => true, 'result' => $result]);
    }

    default:
        ai_api_fail('未知操作：' . $action);
}