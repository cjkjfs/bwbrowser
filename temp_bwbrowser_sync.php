<?php
/**
 * BW Browser 专用 API
 * 完全独立，不修改任何现有 PHP 文件
 * 只依赖同目录下的 config.php 获取数据库连接
 *
 * 安装：把本文件放到 bwbrowser_sync.php 同目录下即可
 *
 * API 列表：
 *   - login                    登录验证
 *   - get_permissions          获取用户权限
 *   - list_proxies             获取代理列表
 *   - sync_proxy               新增/更新代理
 *   - delete_proxy             删除代理
 *   - list_envs                获取环境列表
 *   - sync_env                 新增/更新环境
 *   - delete_env               删除环境
 *   - list_accounts            获取云端账号列表
 *   - get_account_detail       获取账号详情
 */

header('Content-Type: application/json; charset=utf-8');
header('Access-Control-Allow-Origin: *');
header('Access-Control-Allow-Methods: GET, POST, OPTIONS');
header('Access-Control-Allow-Headers: Content-Type, Authorization');

if ($_SERVER['REQUEST_METHOD'] === 'OPTIONS') {
    http_response_code(200);
    exit;
}

// ========== 加载配置 ==========
if (!file_exists(__DIR__ . '/config.php')) {
    json_response(['success' => false, 'message' => '配置文件不存在，请将本文件放到 bwbrowser 根目录'], 500);
}
require_once __DIR__ . '/config.php';

// ========== 日志 ==========
$BWBROWSER_LOG_DIR = __DIR__ . '/logs';
if (!is_dir($BWBROWSER_LOG_DIR)) @mkdir($BWBROWSER_LOG_DIR, 0755, true);
$BWBROWSER_LOG_FILE = $BWBROWSER_LOG_DIR . '/bwbrowser_sync.log';

function bwbrowser_log($msg, $level = 'INFO') {
    global $BWBROWSER_LOG_FILE;
    $ts = date('Y-m-d H:i:s');
    $ip = $_SERVER['REMOTE_ADDR'] ?? 'unknown';
    $line = "[$ts] [$level] [$ip] $msg\n";
    @file_put_contents($BWBROWSER_LOG_FILE, $line, FILE_APPEND | LOCK_EX);
}

function json_response($data, $code = 200) {
    http_response_code($code);
    echo json_encode($data, JSON_UNESCAPED_UNICODE);
    exit;
}

// ========== 工具函数 ==========

/**
 * 验证用户账号密码
 */
function bwbrowser_get_user($pdo, $username, $password) {
    if (empty($username) || empty($password)) return false;

    $stmt = $pdo->prepare("SELECT * FROM users WHERE username = ? LIMIT 1");
    $stmt->execute([$username]);
    $user = $stmt->fetch();
    if (!$user) return false;

    // 验证密码（兼容 password_hash 和 md5）
    if (!empty($user['password']) && function_exists('password_verify') && password_verify($password, $user['password'])) {
        return $user;
    }
    if (!empty($user['password']) && md5($password) === $user['password']) {
        return $user;
    }
    // 兼容其他可能的加密方式
    if (!empty($user['password']) && $user['password'] === $password) {
        return $user;
    }

    return false;
}

/**
 * 判断是否为超级管理员
 */
function bwbrowser_is_super_admin($user) {
    return !empty($user['is_super_admin']);
}

/**
 * 判断是否为经理及以上
 */
function bwbrowser_is_manager($user) {
    if (bwbrowser_is_super_admin($user)) return true;
    return isset($user['role']) && in_array($user['role'], ['manager', 'supervisor', 'leader']);
}

/**
 * 解析模块权限
 * 权限存储在 users 表的独立字段中
 */
function bwbrowser_get_module_perms($user) {
    $perms = [
        'allow_view_revenue' => !empty($user['allow_view_revenue']),
        'allow_view_password' => !empty($user['allow_view_password']),
        'allow_manage_users' => !empty($user['allow_manage_users']),
        'allow_user_management' => !empty($user['allow_user_management']),
        'allow_sms_management' => !empty($user['allow_sms_management']),
        'allow_profile' => !empty($user['allow_profile']),
        'allow_account_detail' => !empty($user['allow_account_detail']),
        'allow_account_revenue' => !empty($user['allow_account_revenue']),
        'allow_data_dashboard' => !empty($user['allow_data_dashboard']),
        'allow_data_report' => !empty($user['allow_data_report']),
        'allow_baowenku' => !empty($user['allow_baowenku']),
        'allow_operation_log' => !empty($user['allow_operation_log']),
        'allow_remote_desktop' => !empty($user['allow_remote_desktop']),
        'allow_remote_management' => !empty($user['allow_remote_management']),
    ];
    // 同时兼容 JSON 方式
    if (!empty($user['module_permissions_json'])) {
        $decoded = json_decode($user['module_permissions_json'], true);
        if (is_array($decoded)) {
            $perms = array_merge($perms, $decoded);
        }
    }
    return $perms;
}

/**
 * 检查权限：模块权限或角色权限
 */
function bwbrowser_check_perm($user, $perm_key, $min_role = 'member') {
    if (bwbrowser_is_super_admin($user)) return true;

    $module_perms = bwbrowser_get_module_perms($user);
    if (isset($module_perms[$perm_key]) && $module_perms[$perm_key]) return true;

    // 角色层级：super_admin > manager > supervisor > member > intern
    $role_level = [
        'super_admin' => 100,
        'manager' => 80,
        'supervisor' => 60,
        'member' => 40,
        'intern' => 20,
    ];
    $user_level = $role_level[$user['role'] ?? 'member'] ?? 0;
    $min_level = $role_level[$min_role] ?? 0;

    return $user_level >= $min_level;
}

// ========== 获取请求参数 ==========
$ACTION = $_POST['action'] ?? $_GET['action'] ?? '';
if (empty($ACTION)) {
    json_response(['success' => false, 'message' => '缺少 action 参数'], 400);
}

bwbrowser_log("=== 请求开始: action=$ACTION ===");

try {
    switch ($ACTION) {

        // ========== 登录 ==========
        case 'login':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';
            $client_version = $_POST['client_version'] ?? 'bwbrowser-unknown';

            if (empty($username) || empty($password)) {
                bwbrowser_log("login: 空参数", 'WARN');
                json_response(['success' => false, 'message' => '用户名和密码不能为空'], 400);
            }

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("login FAILED: user=$username", 'WARN');
                json_response(['success' => false, 'message' => '用户名或密码错误'], 401);
            }

            bwbrowser_log("login OK: user=$username role={$user['role']} company_id={$user['company_id']} version=$client_version");

            // 记录登录日志到 login_logs 表
            $login_ip = $_SERVER['HTTP_X_FORWARDED_FOR'] ?? $_SERVER['REMOTE_ADDR'] ?? 'unknown';
            if (strpos($login_ip, ',') !== false) $login_ip = trim(explode(',', $login_ip)[0]);
            $login_ua = $_SERVER['HTTP_USER_AGENT'] ?? '';
            $login_platform = 'Unknown';
            if (stripos($login_ua, 'Windows') !== false) $login_platform = 'Windows';
            elseif (stripos($login_ua, 'Mac') !== false) $login_platform = 'macOS';
            elseif (stripos($login_ua, 'Linux') !== false) $login_platform = 'Linux';
            // browser 字段显示 client_version（如 "BwBrowser 1.3.9"）
            $login_browser = !empty($client_version) ? $client_version : 'BwBrowser';
            try {
                $login_stmt = $pdo->prepare("INSERT INTO login_logs
                    (user_id, username, real_name, role, company_id, login_time, login_ip, user_agent, platform, browser, device_type, login_result, fail_reason)
                    VALUES (?, ?, ?, ?, ?, NOW(), ?, ?, ?, ?, 'PC', 'success', NULL)");
                $login_stmt->execute([
                    (int)$user['id'], $user['username'], $user['real_name'] ?? $user['username'],
                    $user['role'], (int)$user['company_id'],
                    $login_ip, $login_ua, $login_platform, $login_browser
                ]);
            } catch (Exception $e) {
                bwbrowser_log("login_log insert failed: " . $e->getMessage(), 'ERROR');
            }

            // 获取公司名
            $company_name = '';
            if (!empty($user['company_id'])) {
                $stmt = $pdo->prepare("SELECT company_name FROM companies WHERE id = ?");
                $stmt->execute([$user['company_id']]);
                $company = $stmt->fetch();
                if ($company) $company_name = $company['company_name'];
            }

            // 获取统计数据（简化版）
            $stats = [
                'profiles_count' => 0,
                'proxies_count' => 0,
                'accounts_count' => 0,
            ];
            try {
                $stmt = $pdo->prepare("SELECT COUNT(*) as cnt FROM simprint_environments WHERE user_id = ? OR company_id = ?");
                $stmt->execute([$user['id'], $user['company_id']]);
                $row = $stmt->fetch();
                if ($row) $stats['profiles_count'] = (int)$row['cnt'];
            } catch (Exception $e) {}
            try {
                $stmt = $pdo->prepare("SELECT COUNT(*) as cnt FROM simprint_proxies WHERE user_id = ? OR company_id = ?");
                $stmt->execute([$user['id'], $user['company_id']]);
                $row = $stmt->fetch();
                if ($row) $stats['proxies_count'] = (int)$row['cnt'];
            } catch (Exception $e) {}
            try {
                $stmt = $pdo->prepare("SELECT COUNT(*) as cnt FROM tiktok_accounts WHERE owner_id = ? AND is_deleted = 0");
                $stmt->execute([$user['id']]);
                $row = $stmt->fetch();
                if ($row) $stats['accounts_count'] = (int)$row['cnt'];
            } catch (Exception $e) {}

            json_response([
                'success' => true,
                'user' => [
                    'user_id' => (int)$user['id'],
                    'username' => $user['username'],
                    'real_name' => $user['real_name'] ?? $user['username'],
                    'email' => $user['email'] ?? '',
                    'phone' => $user['phone'] ?? '',
                    'role' => $user['role'],
                    'company_id' => (int)$user['company_id'],
                    'company_name' => $company_name,
                    'avatar_url' => $user['dingtalk_avatar'] ?? null,
                    'is_pro' => true,
                    'plan_name' => bwbrowser_is_super_admin($user) ? '超级管理员' : (bwbrowser_is_manager($user) ? '专业版' : '标准版'),
                    'stats' => $stats,
                ],
            ]);
            break;

        // ========== 获取用户权限 ==========
        case 'get_permissions':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("get_permissions: auth failed for $username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            bwbrowser_log("get_permissions: user={$user['username']} role={$user['role']}");

            $is_super_admin = bwbrowser_is_super_admin($user);
            $is_manager = bwbrowser_is_manager($user);
            $module_perms = bwbrowser_get_module_perms($user);

            json_response([
                'success' => true,
                'permissions' => [
                    // 基础信息
                    'user_id' => (int)$user['id'],
                    'username' => $user['username'],
                    'real_name' => $user['real_name'] ?? $user['username'],
                    'role' => $user['role'],
                    'company_id' => (int)$user['company_id'],
                    'is_super_admin' => $is_super_admin,
                    'is_manager' => $is_manager,
                    // 功能权限
                    'allow_view_password' => bwbrowser_check_perm($user, 'allow_view_password', 'manager'),
                    'allow_proxy_management' => bwbrowser_check_perm($user, 'allow_proxy_management', 'manager'),
                    'allow_env_management' => true,
                    'allow_view_revenue' => bwbrowser_check_perm($user, 'allow_view_revenue', 'manager'),
                    'allow_manage_users' => $is_super_admin || ($user['role'] === 'manager'),
                    'allow_cloud_accounts' => true,
                    'allow_cookie_management' => true,
                    'allow_export' => bwbrowser_check_perm($user, 'allow_export', 'member'),
                    // 原始模块权限
                    'module_permissions' => $module_perms,
                ],
            ]);
            break;

        // ========== 代理列表 ==========
        case 'list_proxies':
            // 自动添加缺失字段（如果不存在）
            try {
                $pdo->exec("ALTER TABLE simprint_proxies ADD COLUMN IF NOT EXISTS timezone VARCHAR(64) DEFAULT NULL");
            } catch (Exception $e) {
                // 字段已存在或无权限，忽略
            }
            try {
                $pdo->exec("ALTER TABLE simprint_proxies ADD COLUMN IF NOT EXISTS protocol_config TEXT DEFAULT NULL");
            } catch (Exception $e) {
                // 字段已存在或无权限，忽略
            }
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("list_proxies: auth failed for $username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            $is_manager = bwbrowser_is_manager($user);
            $is_super_admin = bwbrowser_is_super_admin($user);

            // 构建查询
            $where = [];
            $params = [];
            $target_company_id = (int)($_POST['company_id'] ?? 0);
            if ($is_super_admin) {
                // 超级管理员可以切换公司
                $where[] = "company_id = ?";
                $params[] = $target_company_id > 0 ? $target_company_id : $user['company_id'];
            } elseif ($is_manager) {
                // 经理看自己和下属
                $where[] = "(company_id = ? AND (user_id = ? OR user_id IN (SELECT id FROM users WHERE parent_id = ?)))";
                $params[] = $user['company_id'];
                $params[] = $user['id'];
                $params[] = $user['id'];
            } else {
                // 普通用户看整个公司代理
                $where[] = "company_id = ?";
                $params[] = $user['company_id'];
            }

            $where_sql = implode(' AND ', $where);
            $sql = "SELECT * FROM simprint_proxies WHERE $where_sql ORDER BY id DESC";

            $stmt = $pdo->prepare($sql);
            $stmt->execute($params);
            $proxies = $stmt->fetchAll();

            bwbrowser_log("list_proxies: user={$user['username']} count=" . count($proxies));

            $result = array_map(function($p) {
                return [
                    'id' => (int)$p['id'],
                    'name' => $p['name'],
                    'proxy_type' => $p['proxy_type'],
                    'host' => $p['host'],
                    'port' => (int)$p['port'],
                    'username' => $p['username'] ?? null,
                    'password' => $p['password'] ?? null,
                    'country' => $p['country'] ?? null,
                    'city' => $p['city'] ?? null,
                    'timezone' => $p['timezone'] ?? null,
                    'protocol_config' => $p['protocol_config'] ?? null,
                    'proxy_uuid' => $p['proxy_uuid'] ?? null,
                    'user_id' => (int)$p['user_id'],
                    'company_id' => (int)$p['company_id'],
                    'created_at' => $p['created_at'] ?? null,
                    'updated_at' => $p['updated_at'] ?? null,
                ];
            }, $proxies);

            json_response([
                'success' => true,
                'proxies' => $result,
                'total' => count($result),
            ]);
            break;

        // ========== 同步代理（新增/更新） ==========
        case 'sync_proxy':
            // 确保 timezone 和 protocol_config 字段存在
            try {
                $pdo->exec("ALTER TABLE simprint_proxies ADD COLUMN IF NOT EXISTS timezone VARCHAR(64) DEFAULT NULL");
            } catch (Exception $e) {
                // 字段已存在或无权限，忽略
            }
            try {
                $pdo->exec("ALTER TABLE simprint_proxies ADD COLUMN IF NOT EXISTS protocol_config TEXT DEFAULT NULL");
            } catch (Exception $e) {
                // 字段已存在或无权限，忽略
            }

            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("sync_proxy: auth failed for $username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            $proxy_id = !empty($_POST['proxy_id']) ? (int)$_POST['proxy_id'] : null;
            $proxy_name = trim($_POST['proxy_name'] ?? '');
            $proxy_type = $_POST['proxy_type'] ?? 'http';
            $host = trim($_POST['host'] ?? '');
            $port = (int)($_POST['port'] ?? 0);
            $proxy_username = $_POST['username_proxy'] ?? null;
            $proxy_password = $_POST['password_proxy'] ?? null;
            $country = $_POST['country'] ?? null;
            $city = $_POST['city'] ?? null;
            $timezone = $_POST['timezone'] ?? null;
            $protocol_config = $_POST['protocol_config'] ?? null;

            // 高级协议：如果 host/port 为空但 protocol_config 有值，从 URI 中解析
            if (in_array(strtolower($proxy_type), ['vless', 'trojan', 'ss']) && !empty($protocol_config)) {
                if (empty($host) || $port <= 0) {
                    // 尝试从 URI 中解析 host 和 port
                    if (preg_match('/^[a-z]+:\/\/[^@]+@([^:?#]+):(\d+)/i', $protocol_config, $m)) {
                        if (empty($host)) $host = $m[1];
                        if ($port <= 0) $port = (int)$m[2];
                    }
                }
            }

            if (empty($proxy_name) || empty($host) || $port <= 0) {
                json_response(['success' => false, 'message' => '代理名称、地址、端口不能为空'], 400);
            }

            $is_manager = bwbrowser_is_manager($user);

            if ($proxy_id) {
                // 更新
                // 检查权限
                $stmt = $pdo->prepare("SELECT * FROM simprint_proxies WHERE id = ?");
                $stmt->execute([$proxy_id]);
                $existing = $stmt->fetch();
                if (!$existing) {
                    json_response(['success' => false, 'message' => '代理不存在'], 404);
                }
                if (!$is_manager && $existing['user_id'] != $user['id']) {
                    bwbrowser_log("sync_proxy: 权限不足 user={$user['username']} proxy_id=$proxy_id", 'WARN');
                    json_response(['success' => false, 'message' => '权限不足'], 403);
                }

                $stmt = $pdo->prepare("UPDATE simprint_proxies SET name=?, proxy_type=?, host=?, port=?, username=?, password=?, country=?, city=?, timezone=?, protocol_config=?, updated_at=NOW() WHERE id=?");
                $stmt->execute([
                    $proxy_name, $proxy_type, $host, $port,
                    $proxy_username, $proxy_password, $country, $city, $timezone, $protocol_config,
                    $proxy_id
                ]);
                bwbrowser_log("sync_proxy UPDATE OK: id=$proxy_id name=$proxy_name user={$user['username']}");
            } else {
                // 新增
                $proxy_uuid = $_POST['proxy_uuid'] ?? null;
                if (empty($proxy_uuid)) {
                    $proxy_uuid = sprintf('%04x%04x-%04x-%04x-%04x-%04x%04x%04x',
                        mt_rand(0, 0xffff), mt_rand(0, 0xffff),
                        mt_rand(0, 0xffff),
                        mt_rand(0, 0x0fff) | 0x4000,
                        mt_rand(0, 0x3fff) | 0x8000,
                        mt_rand(0, 0xffff), mt_rand(0, 0xffff), mt_rand(0, 0xffff)
                    );
                }

                $stmt = $pdo->prepare("INSERT INTO simprint_proxies (proxy_uuid, name, proxy_type, host, port, username, password, country, city, timezone, protocol_config, user_id, user_name, company_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NOW(), NOW())");
                $stmt->execute([
                    $proxy_uuid, $proxy_name, $proxy_type, $host, $port,
                    $proxy_username, $proxy_password, $country, $city, $timezone, $protocol_config,
                    $user['id'], $user['username'], $user['company_id']
                ]);
                $proxy_id = (int)$pdo->lastInsertId();
                bwbrowser_log("sync_proxy INSERT OK: id=$proxy_id name=$proxy_name user={$user['username']}");
            }

            json_response([
                'success' => true,
                'proxy_id' => $proxy_id,
                'message' => $proxy_id ? '代理已更新' : '代理已创建',
            ]);
            break;

        // ========== 删除代理 ==========
        case 'delete_proxy':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("delete_proxy: auth failed for $username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            $proxy_id = (int)($_POST['proxy_id'] ?? 0);
            if (!$proxy_id) {
                json_response(['success' => false, 'message' => 'proxy_id 不能为空'], 400);
            }

            $is_manager = bwbrowser_is_manager($user);

            // 检查权限
            $stmt = $pdo->prepare("SELECT * FROM simprint_proxies WHERE id = ?");
            $stmt->execute([$proxy_id]);
            $existing = $stmt->fetch();
            if (!$existing) {
                json_response(['success' => false, 'message' => '代理不存在'], 404);
            }
            if (!$is_manager && $existing['user_id'] != $user['id']) {
                bwbrowser_log("delete_proxy: 权限不足 user={$user['username']} proxy_id=$proxy_id", 'WARN');
                json_response(['success' => false, 'message' => '权限不足'], 403);
            }

            $stmt = $pdo->prepare("DELETE FROM simprint_proxies WHERE id = ?");
            $stmt->execute([$proxy_id]);
            bwbrowser_log("delete_proxy OK: id=$proxy_id user={$user['username']}");

            json_response(['success' => true, 'message' => '代理已删除']);
            break;

        // ========== 环境列表 ==========
        case 'list_envs':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("list_envs: auth failed for $username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            $is_manager = bwbrowser_is_manager($user);
            $is_super_admin = bwbrowser_is_super_admin($user);

            $where = [];
            $params = [];
            $target_company_id = (int)($_POST['company_id'] ?? 0);
            if ($is_super_admin) {
                // 超级管理员可以切换公司，默认看自己公司
                $where[] = "company_id = ?";
                $params[] = $target_company_id > 0 ? $target_company_id : $user['company_id'];
            } elseif ($is_manager) {
                $where[] = "(company_id = ? AND (user_id = ? OR user_id IN (SELECT id FROM users WHERE parent_id = ?)))";
                $params[] = $user['company_id'];
                $params[] = $user['id'];
                $params[] = $user['id'];
            } else {
                $where[] = "user_id = ?";
                $params[] = $user['id'];
            }

            $where_sql = implode(' AND ', $where);
            $sql = "SELECT * FROM simprint_environments WHERE $where_sql ORDER BY id DESC";

            $stmt = $pdo->prepare($sql);
            $stmt->execute($params);
            $envs = $stmt->fetchAll();

            bwbrowser_log("list_envs: user={$user['username']} count=" . count($envs));

            $result = array_map(function($e) {
                $fingerprint = null;
                if (!empty($e['fingerprint_config'])) {
                    // 直接返回 JSON 字符串，Rust 端用字符串存储
                    $fingerprint = is_string($e['fingerprint_config']) ? $e['fingerprint_config'] : json_encode($e['fingerprint_config']);
                }
                $start_urls = null;
                if (!empty($e['start_urls'])) {
                    $start_urls = json_decode($e['start_urls'], true);
                    if (!is_array($start_urls)) $start_urls = null;
                }
                return [
                    'env_uuid' => $e['env_uuid'],
                    'name' => $e['name'],
                    'description' => $e['description'] ?? null,
                    'browser_type' => $e['browser_type'] ?? 'chromium',
                    'status' => $e['status'] ?? 'ready',
                    'fingerprint_config' => $fingerprint,
                    'start_urls' => $start_urls,
                    'remark' => $e['remark'] ?? null,
                    'user_id' => (int)$e['user_id'],
                    'owner_name' => null,
                    'company_id' => (int)$e['company_id'],
                    'group_uuid' => $e['group_uuid'] ?? null,
                    'proxy_uuid' => $e['proxy_uuid'] ?? null,
                    'tag_uuids' => $e['tag_uuids'] ? json_decode($e['tag_uuids'], true) : null,
                    'updated_at' => $e['updated_at'] ?? null,
                    'created_at' => $e['created_at'] ?? null,
                ];
            }, $envs);

            json_response([
                'success' => true,
                'environments' => $result,
                'total' => count($result),
            ]);
            break;

        // ========== 同步环境（新增/更新） ==========
        case 'sync_env':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("sync_env: auth failed for $username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            $env_uuid = trim($_POST['env_uuid'] ?? '');
            $name = trim($_POST['name'] ?? '');
            $description = $_POST['description'] ?? null;
            $browser_type = $_POST['browser_type'] ?? 'chromium';
            $fingerprint_config = $_POST['fingerprint_config'] ?? null;
            $start_urls = $_POST['start_urls'] ?? null;
            $remark = $_POST['remark'] ?? null;
            $status = $_POST['status'] ?? 'ready';

            if (empty($env_uuid) || empty($name)) {
                json_response(['success' => false, 'message' => 'env_uuid 和环境名称不能为空'], 400);
            }

            $is_manager = bwbrowser_is_manager($user);

            // 检查是否已存在
            $stmt = $pdo->prepare("SELECT * FROM simprint_environments WHERE env_uuid = ?");
            $stmt->execute([$env_uuid]);
            $existing = $stmt->fetch();

            if ($existing) {
                // 更新
                if (!$is_manager && $existing['user_id'] != $user['id']) {
                    bwbrowser_log("sync_env: 权限不足 user={$user['username']} env_uuid=$env_uuid", 'WARN');
                    json_response(['success' => false, 'message' => '权限不足'], 403);
                }

                $stmt = $pdo->prepare("UPDATE simprint_environments SET name=?, description=?, browser_type=?, fingerprint_config=?, start_urls=?, remark=?, status=?, updated_at=NOW() WHERE env_uuid=?");
                $stmt->execute([
                    $name, $description, $browser_type,
                    $fingerprint_config, $start_urls, $remark, $status,
                    $env_uuid
                ]);
                bwbrowser_log("sync_env UPDATE OK: uuid=$env_uuid name=$name user={$user['username']}");
            } else {
                // 新增
                $stmt = $pdo->prepare("INSERT INTO simprint_environments (env_uuid, name, description, browser_type, fingerprint_config, start_urls, remark, status, user_id, company_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NOW(), NOW())");
                $stmt->execute([
                    $env_uuid, $name, $description, $browser_type,
                    $fingerprint_config, $start_urls, $remark, $status,
                    $user['id'], $user['company_id']
                ]);
                bwbrowser_log("sync_env INSERT OK: uuid=$env_uuid name=$name user={$user['username']}");
            }

            json_response([
                'success' => true,
                'env_uuid' => $env_uuid,
                'message' => $existing ? '环境已更新' : '环境已创建',
            ]);
            break;

        // ========== 删除环境 ==========
        case 'delete_env':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("delete_env: auth failed for $username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            $env_uuid = trim($_POST['env_uuid'] ?? '');
            if (empty($env_uuid)) {
                json_response(['success' => false, 'message' => 'env_uuid 不能为空'], 400);
            }

            $is_manager = bwbrowser_is_manager($user);

            $stmt = $pdo->prepare("SELECT * FROM simprint_environments WHERE env_uuid = ?");
            $stmt->execute([$env_uuid]);
            $existing = $stmt->fetch();
            if (!$existing) {
                json_response(['success' => false, 'message' => '环境不存在'], 404);
            }
            if (!$is_manager && $existing['user_id'] != $user['id']) {
                bwbrowser_log("delete_env: 权限不足 user={$user['username']} env_uuid=$env_uuid", 'WARN');
                json_response(['success' => false, 'message' => '权限不足'], 403);
            }

            $pdo->prepare("DELETE FROM simprint_environments WHERE env_uuid = ?")->execute([$env_uuid]);
            // 同时清理关联的 cookies
            $pdo->prepare("DELETE FROM simprint_env_cookies WHERE env_uuid = ?")->execute([$env_uuid]);
            // 清理账号的 env_uuid 引用
            $pdo->prepare("UPDATE tiktok_accounts SET env_uuid = NULL WHERE env_uuid = ?")->execute([$env_uuid]);

            bwbrowser_log("delete_env OK: uuid=$env_uuid user={$user['username']}");
            json_response(['success' => true, 'message' => '环境已删除']);
            break;

        // ========== 云端账号列表 ==========
        case 'list_accounts':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';
            $page = max(1, (int)($_POST['page'] ?? 1));
            $page_size = min(100, max(1, (int)($_POST['page_size'] ?? 20)));
            $platform = $_POST['platform'] ?? '';
            $keyword = $_POST['keyword'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("list_accounts: auth failed for $username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            $is_manager = bwbrowser_is_manager($user);
            $is_super_admin = bwbrowser_is_super_admin($user);

            $where = ["is_deleted = 0"];
            $params = [];

            $target_company_id = (int)($_POST['company_id'] ?? 0);
            if ($is_super_admin) {
                // 超级管理员可以切换公司，默认看自己公司
                $where[] = "company_id = ?";
                $params[] = $target_company_id > 0 ? $target_company_id : $user['company_id'];
            } elseif ($is_manager) {
                // 管理员看整个公司的账号（与 simprint_accounts.php 逻辑一致）
                $where[] = "company_id = ?";
                $params[] = $target_company_id > 0 ? $target_company_id : $user['company_id'];
            } else {
                $where[] = "owner_id = ?";
                $params[] = $user['id'];
            }

            // 只有显式传了 owner_id 才按归属人过滤
            $filter_owner_id = (int)($_POST['owner_id'] ?? 0);
            if ($filter_owner_id > 0) {
                $where[] = "owner_id = ?";
                $params[] = $filter_owner_id;
            }

            if (!empty($platform)) {
                $where[] = "platform = ?";
                $params[] = $platform;
            }

            if (!empty($keyword)) {
                $where[] = "(phone_id LIKE ? OR account_nickname LIKE ? OR login_account LIKE ?)";
                $kw = "%$keyword%";
                $params[] = $kw;
                $params[] = $kw;
                $params[] = $kw;
            }

            $where_sql = implode(' AND ', $where);

            // 统计总数
            $stmt = $pdo->prepare("SELECT COUNT(*) as cnt FROM tiktok_accounts WHERE $where_sql");
            $stmt->execute($params);
            $total_row = $stmt->fetch();
            $total = (int)($total_row['cnt'] ?? 0);

            // 分页查询
            $offset = ($page - 1) * $page_size;
            $sql = "SELECT * FROM tiktok_accounts WHERE $where_sql ORDER BY id DESC LIMIT $offset, $page_size";
            $stmt = $pdo->prepare($sql);
            $stmt->execute($params);
            $accounts = $stmt->fetchAll();

            bwbrowser_log("list_accounts: user={$user['username']} total=$total page=$page page_size=$page_size");

            $result = array_map(function($a) use ($user, $pdo) {
                // 获取归属人名字
                $owner_name = '';
                if (!empty($a['owner_id'])) {
                    try {
                        $stmt = $pdo->prepare("SELECT real_name, username FROM users WHERE id = ?");
                        $stmt->execute([$a['owner_id']]);
                        $u = $stmt->fetch();
                        if ($u) $owner_name = $u['real_name'] ?: $u['username'];
                    } catch (Exception $e) {}
                }
                // 获取代理信息
                $proxy_info = '';
                if (!empty($a['proxy_id'])) {
                    try {
                        $stmt = $pdo->prepare("SELECT name, host, port FROM simprint_proxies WHERE id = ?");
                        $stmt->execute([$a['proxy_id']]);
                        $p = $stmt->fetch();
                        if ($p) $proxy_info = "{$p['host']}:{$p['port']}";
                    } catch (Exception $e) {}
                }
                return [
                    'id' => (int)$a['id'],
                    'account_name' => $a['account_name'] ?? '',
                    'phone_id' => $a['phone_id'] ?? '',
                    'account_nickname' => $a['account_nickname'] ?? '',
                    'nickname' => $a['account_nickname'] ?? '',
                    'login_account' => $a['login_account'] ?? '',
                    'login_password' => $a['login_password'] ?? '',
                    'platform' => $a['platform'] ?? '',
                    'safe_link' => $a['safe_link'] ?? null,
                    'bind_phone' => $a['bind_phone'] ?? null,
                    'backup_email' => $a['backup_email'] ?? null,
                    'owner_id' => isset($a['owner_id']) ? (int)$a['owner_id'] : null,
                    'owner_name' => $owner_name,
                    'env_uuid' => $a['env_uuid'] ?? null,
                    'proxy_id' => $a['proxy_id'] ?? null,
                    'proxy_node' => $proxy_info,
                    'last_login_ip' => $a['last_login_ip'] ?? '',
                    'cookie_updated_at' => $a['cookie_updated_at'] ?? null,
                    'remark' => $a['remark'] ?? '',
                    'tags' => $a['tags_json'] ? json_decode($a['tags_json'], true) : [],
                    'tags_list' => $a['tags_json'] ? json_decode($a['tags_json'], true) : [],
                    'status' => isset($a['status']) ? (int)$a['status'] : 1,
                    'created_at' => $a['created_at'] ?? null,
                    'updated_at' => $a['updated_at'] ?? null,
                ];
            }, $accounts);

            json_response([
                'success' => true,
                'accounts' => $result,
                'total' => $total,
                'page' => $page,
                'page_size' => $page_size,
                'total_pages' => ceil($total / $page_size),
                'is_manager' => $is_manager,
                'is_super_admin' => $is_super_admin,
                'can_view_password' => bwbrowser_check_perm($user, 'allow_view_password', 'manager'),
                'can_view_2fa' => bwbrowser_check_perm($user, 'allow_account_detail', 'member'),
                'can_view_sms' => bwbrowser_check_perm($user, 'allow_sms_management', 'member'),
            ]);
            break;

        // ========== 获取账号详情 ==========
        case 'get_account_detail':
            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) {
                json_response(['success' => false, 'message' => 'account_id required'], 400);
            }

            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';
            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("get_account_detail: auth failed for $username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            // JOIN account_details 表，确保网页端 account_detail_admin.php 看到的字段一致
            $stmt = $pdo->prepare("
                SELECT a.*, ad.device_id as ad_device_id, ad.login_account as ad_login_account,
                       ad.login_password as ad_login_password, ad.bind_phone as ad_bind_phone,
                       ad.safe_link as ad_safe_link, ad.backup_email as ad_backup_email,
                       ad.bind_email as ad_bind_email, ad.email_password as ad_email_password,
                       ad.node_ip as ad_node_ip
                FROM tiktok_accounts a
                LEFT JOIN account_details ad ON ad.account_id = a.id
                WHERE a.id = ? AND a.is_deleted = 0
            ");
            $stmt->execute([$account_id]);
            $account = $stmt->fetch();

            if (!$account) {
                json_response(['success' => false, 'message' => '账号不存在'], 404);
            }

            // 权限校验：非管理员只能看自己的
            $is_manager = bwbrowser_is_manager($user);
            $is_super_admin = bwbrowser_is_super_admin($user);
            $account_owner = $account['owner_id'] ?? $account['user_id'] ?? null;
            if (!$is_super_admin && !$is_manager && $account_owner != $user['id']) {
                json_response(['success' => false, 'message' => '无权限查看此账号'], 403);
            }

            // 归属人名字
            $owner_name = '';
            if (!empty($account_owner)) {
                try {
                    $stmt2 = $pdo->prepare("SELECT real_name, username FROM users WHERE id = ?");
                    $stmt2->execute([$account_owner]);
                    $u = $stmt2->fetch();
                    if ($u) $owner_name = $u['real_name'] ?: $u['username'];
                } catch (Exception $e) {}
            }

            $account['owner_name'] = $owner_name;
            $account['account_nickname'] = $account['account_nickname'] ?? $account['nickname'] ?? '';

            bwbrowser_log("get_account_detail: id={$account_id} user={$user['username']}");

            json_response([
                'success' => true,
                'account' => $account,
            ]);
            break;

        // ========== 获取账号 Cookie ==========
        case 'get_account_cookies':
            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) {
                json_response(['success' => false, 'message' => 'account_id required'], 400);
            }
            $stmt = $pdo->prepare("SELECT cookie, cookie_updated_at FROM tiktok_accounts WHERE id = ? AND is_deleted = 0");
            $stmt->execute([$account_id]);
            $row = $stmt->fetch();
            if (!$row) {
                json_response(['success' => false, 'message' => '账号不存在'], 404);
            }
            json_response([
                'success' => true,
                'cookie' => $row['cookie'] ?? null,
                'cookie_updated_at' => $row['cookie_updated_at'] ?? null,
            ]);
            break;

        // ========== 更新账号 Cookie ==========
        case 'update_account_cookies':
            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) {
                json_response(['success' => false, 'message' => 'account_id required'], 400);
            }
            $cookie = $_POST['cookie'] ?? null;
            $stmt = $pdo->prepare("UPDATE tiktok_accounts SET cookie = ?, cookie_updated_at = NOW(), updated_at = NOW() WHERE id = ?");
            $stmt->execute([$cookie, $account_id]);
            bwbrowser_log("update_account_cookies: id=$account_id cookie=" . ($cookie ? 'yes' : 'no'));
            json_response(['success' => true]);
            break;

        // ========== 删除账号 Cookie ==========
        case 'delete_account_cookies':
            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) {
                json_response(['success' => false, 'message' => 'account_id required'], 400);
            }
            $stmt = $pdo->prepare("UPDATE tiktok_accounts SET cookie = NULL, cookie_updated_at = NULL, updated_at = NOW() WHERE id = ?");
            $stmt->execute([$account_id]);
            bwbrowser_log("delete_account_cookies: id=$account_id");
            json_response(['success' => true]);
            break;

        // ========== 更新账号 2FA/短信链接 ==========
        case 'update_account_codes':
            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) {
                json_response(['success' => false, 'message' => 'account_id required'], 400);
            }
            $safe_link = $_POST['safe_link'] ?? null;
            $bind_phone = $_POST['bind_phone'] ?? null;
            $stmt = $pdo->prepare("UPDATE tiktok_accounts SET safe_link = ?, bind_phone = ?, updated_at = NOW() WHERE id = ?");
            $stmt->execute([$safe_link, $bind_phone, $account_id]);
            bwbrowser_log("update_account_codes: id=$account_id safe_link=" . ($safe_link ? 'yes' : 'no') . " bind_phone=" . ($bind_phone ? 'yes' : 'no'));
            json_response(['success' => true]);
            break;

        // ========== 更新账号环境绑定 ==========
        case 'update_account_env':
            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) {
                json_response(['success' => false, 'message' => 'account_id required'], 400);
            }
            $env_uuid = $_POST['env_uuid'] ?? null;
            $stmt = $pdo->prepare("UPDATE tiktok_accounts SET env_uuid = ?, updated_at = NOW() WHERE id = ?");
            $stmt->execute([$env_uuid, $account_id]);
            bwbrowser_log("update_account_env: id=$account_id env_uuid=" . ($env_uuid ?: 'null'));
            json_response(['success' => true]);
            break;

        // ========== 更新账号信息 ==========
        case 'update_account_info':
            // 记录所有收到的 POST 数据（密码除外）
            $debug_post = $_POST;
            if (isset($debug_post['password'])) $debug_post['password'] = '***';
            bwbrowser_log("update_account_info: 收到POST数据: " . json_encode($debug_post, JSON_UNESCAPED_UNICODE));

            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';
            $account_id = (int)($_POST['account_id'] ?? 0);
            $owner_id = isset($_POST['owner_id']) ? (int)$_POST['owner_id'] : null;
            $remark = isset($_POST['remark']) ? trim($_POST['remark']) : null;
            $tags = $_POST['tags'] ?? null;
            $platform = isset($_POST['platform']) ? trim($_POST['platform']) : null;
            $login_account = isset($_POST['login_account']) ? trim($_POST['login_account']) : null;
            $login_password = isset($_POST['login_password']) ? $_POST['login_password'] : null;
            $bind_phone = isset($_POST['bind_phone']) ? trim($_POST['bind_phone']) : null;
            $safe_link = isset($_POST['safe_link']) ? trim($_POST['safe_link']) : null;
            $phone_id = isset($_POST['phone_id']) ? trim($_POST['phone_id']) : null;
            $nickname = isset($_POST['nickname']) ? trim($_POST['nickname']) : null;
            $account_name = isset($_POST['account_name']) ? trim($_POST['account_name']) : null;
            $category = isset($_POST['category']) ? trim($_POST['category']) : null;
            $status = isset($_POST['status']) ? (int)$_POST['status'] : null;
            $backup_email = isset($_POST['backup_email']) ? trim($_POST['backup_email']) : null;

            bwbrowser_log("update_account_info: 解析后字段: account_id=$account_id, login_account=" . ($login_account ?? 'NULL') . ", login_password=" . ($login_password ? '***' : 'NULL') . ", phone_id=" . ($phone_id ?? 'NULL') . ", bind_phone=" . ($bind_phone ?? 'NULL') . ", safe_link=" . ($safe_link ?? 'NULL') . ", nickname=" . ($nickname ?? 'NULL') . ", account_name=" . ($account_name ?? 'NULL') . ", owner_id=" . ($owner_id ?? 'NULL') . ", backup_email=" . ($backup_email ?? 'NULL'));

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                bwbrowser_log("update_account_info: 认证失败 user=$username", 'WARN');
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }
            if ($account_id <= 0) {
                json_response(['success' => false, 'message' => 'account_id required'], 400);
            }

            $sets = [];
            $params = [];
            if ($owner_id !== null) { $sets[] = 'owner_id = ?'; $params[] = $owner_id; }
            if ($remark !== null) { $sets[] = 'remark = ?'; $params[] = $remark; }
            if ($tags !== null) { $sets[] = 'tags_json = ?'; $params[] = $tags; }
            if ($platform !== null) { $sets[] = 'platform = ?'; $params[] = $platform; }
            if ($login_account !== null) {
                $sets[] = 'login_account = ?';
                $params[] = $login_account;
                $sets[] = 'pure_username = ?';
                $params[] = $login_account;
            }
            if ($login_password !== null) { $sets[] = 'login_password = ?'; $params[] = $login_password; }
            if ($bind_phone !== null) { $sets[] = 'bind_phone = ?'; $params[] = $bind_phone; }
            if ($safe_link !== null) { $sets[] = 'safe_link = ?'; $params[] = $safe_link; }
            if ($phone_id !== null) { $sets[] = 'phone_id = ?'; $params[] = $phone_id; }
            if ($nickname !== null) { $sets[] = 'account_nickname = ?'; $params[] = $nickname; }
            if ($account_name !== null) { $sets[] = 'account_name = ?'; $params[] = $account_name; }
            if ($category !== null) { $sets[] = 'category = ?'; $params[] = $category; }
            if ($status !== null) { $sets[] = 'status = ?'; $params[] = $status; }
            if ($backup_email !== null) { $sets[] = 'backup_email = ?'; $params[] = $backup_email; }

            if (empty($sets)) {
                json_response(['success' => false, 'message' => '没有要更新的字段'], 400);
            }

            $sets[] = 'updated_at = NOW()';
            $params[] = $account_id;
            $sql = "UPDATE tiktok_accounts SET " . implode(', ', $sets) . " WHERE id = ?";
            bwbrowser_log("update_account_info: SQL=$sql");
            bwbrowser_log("update_account_info: params=" . json_encode($params, JSON_UNESCAPED_UNICODE));
            $stmt = $pdo->prepare($sql);
            $stmt->execute($params);
            $affected = $stmt->rowCount();
            bwbrowser_log("update_account_info: id=$account_id affected=$affected fields=" . implode(',', $sets));

            // ========== 同步更新 account_details 表 ==========
            // account_detail_admin.php 通过 JOIN account_details 读取数据，
            // 如果不同步更新此表，网页端看到的 login_account/login_password/bind_phone/safe_link 等会是空的
            $ad_sets = [];
            $ad_params = [];
            if ($login_account !== null) { $ad_sets[] = 'login_account = ?'; $ad_params[] = $login_account; }
            if ($login_password !== null) { $ad_sets[] = 'login_password = ?'; $ad_params[] = $login_password; }
            if ($bind_phone !== null) { $ad_sets[] = 'bind_phone = ?'; $ad_params[] = $bind_phone; }
            if ($safe_link !== null) { $ad_sets[] = 'safe_link = ?'; $ad_params[] = $safe_link; }
            if ($backup_email !== null) { $ad_sets[] = 'backup_email = ?'; $ad_params[] = $backup_email; }
            if ($phone_id !== null) { $ad_sets[] = 'device_id = ?'; $ad_params[] = $phone_id; }

            if (!empty($ad_sets)) {
                // 检查 account_details 是否已有记录
                $check_ad = $pdo->prepare("SELECT id FROM account_details WHERE account_id = ?");
                $check_ad->execute([$account_id]);
                $ad_row = $check_ad->fetch();

                if ($ad_row) {
                    // 已有记录 → UPDATE
                    $ad_sets[] = 'updated_at = NOW()';
                    $ad_params[] = $account_id;
                    $ad_sql = "UPDATE account_details SET " . implode(', ', $ad_sets) . " WHERE account_id = ?";
                    bwbrowser_log("update_account_info: account_details UPDATE SQL=$ad_sql");
                    $ad_stmt = $pdo->prepare($ad_sql);
                    $ad_stmt->execute($ad_params);
                    bwbrowser_log("update_account_info: account_details updated, affected=" . $ad_stmt->rowCount());
                } else {
                    // 没有记录 → INSERT
                    $ad_cols = ['account_id'];
                    $ad_vals = [$account_id];
                    $ad_ph = ['?'];
                    foreach ($ad_sets as $s) {
                        $col = trim(explode('=', $s)[0]);
                        $ad_cols[] = $col;
                        $ad_ph[] = '?';
                    }
                    // 把 ad_params 的值加上（ad_sets 的参数已经按顺序在 ad_params 里了）
                    $ad_sql = "INSERT INTO account_details (" . implode(', ', $ad_cols) . ", created_at) VALUES (" . implode(', ', $ad_ph) . ", NOW())";
                    bwbrowser_log("update_account_info: account_details INSERT SQL=$ad_sql");
                    $ad_stmt = $pdo->prepare($ad_sql);
                    $ad_stmt->execute($ad_params);
                    bwbrowser_log("update_account_info: account_details inserted, id=" . $pdo->lastInsertId());
                }
            }

            // 返回更新后的数据（从两个表 JOIN 查询）
            $check_stmt = $pdo->prepare("
                SELECT a.*, ad.device_id as ad_device_id, ad.login_account as ad_login_account,
                       ad.login_password as ad_login_password, ad.bind_phone as ad_bind_phone,
                       ad.safe_link as ad_safe_link, ad.backup_email as ad_backup_email,
                       ad.bind_email as ad_bind_email, ad.email_password as ad_email_password,
                       ad.node_ip as ad_node_ip
                FROM tiktok_accounts a
                LEFT JOIN account_details ad ON ad.account_id = a.id
                WHERE a.id = ?
            ");
            $check_stmt->execute([$account_id]);
            $updated = $check_stmt->fetch();

            json_response([
                'success' => true,
                'affected' => $affected,
                'updated_fields' => $sets,
                'account' => [
                    'id' => (int)$updated['id'],
                    'account_name' => $updated['account_name'] ?? '',
                    'login_account' => $updated['ad_login_account'] ?? $updated['login_account'] ?? '',
                    'login_password' => $updated['ad_login_password'] ?? $updated['login_password'] ?? '',
                    'phone_id' => $updated['ad_device_id'] ?? $updated['phone_id'] ?? '',
                    'bind_phone' => $updated['ad_bind_phone'] ?? $updated['bind_phone'] ?? '',
                    'safe_link' => $updated['ad_safe_link'] ?? $updated['safe_link'] ?? '',
                    'nickname' => $updated['account_nickname'] ?? '',
                    'owner_id' => isset($updated['owner_id']) ? (int)$updated['owner_id'] : null,
                    'remark' => $updated['remark'] ?? '',
                    'backup_email' => $updated['ad_backup_email'] ?? $updated['backup_email'] ?? '',
                    'category' => $updated['category'] ?? '',
                ],
            ]);
            break;

        // ========== 视频下载日志 ==========
        case 'log_video_download':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';
            $vd_url = $_POST['url'] ?? '';
            $vd_result = $_POST['result'] ?? 'unknown';
            $vd_resolution = $_POST['resolution'] ?? '';
            $vd_file_size = intval($_POST['file_size'] ?? 0);
            $vd_platform = $_POST['platform'] ?? '';
            $vd_output_dir = $_POST['output_dir'] ?? '';
            $vd_message = $_POST['message'] ?? '';
            $vd_title = $_POST['title'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            $log_ip = $_SERVER['HTTP_X_FORWARDED_FOR'] ?? $_SERVER['REMOTE_ADDR'] ?? 'unknown';
            if (strpos($log_ip, ',') !== false) $log_ip = trim(explode(',', $log_ip)[0]);

            $new_value = json_encode([
                'url' => $vd_url,
                'result' => $vd_result,
                'resolution' => $vd_resolution,
                'file_size' => $vd_file_size,
                'platform' => $vd_platform,
                'output_dir' => $vd_output_dir,
                'message' => $vd_message,
                'title' => $vd_title,
            ], JSON_UNESCAPED_UNICODE);

            try {
                $stmt = $pdo->prepare("INSERT INTO operation_logs
                    (company_id, user_id, user_name, user_real_name, action, target_type, target_id, target_name, old_value, new_value, ip_address, created_at)
                    VALUES (?, ?, ?, ?, 'download', 'video_download', ?, ?, NULL, ?, ?, NOW())");
                $stmt->execute([
                    (int)$user['company_id'],
                    (int)$user['id'],
                    $user['username'],
                    $user['real_name'] ?? $user['username'],
                    $vd_url,
                    $vd_title ?: $vd_url,
                    $new_value,
                    $log_ip,
                ]);
                bwbrowser_log("log_video_download: url=$vd_url result=$vd_result user={$user['username']}");
                json_response(['success' => true]);
            } catch (Exception $e) {
                bwbrowser_log("log_video_download failed: " . $e->getMessage(), 'ERROR');
                json_response(['success' => false, 'message' => '记录失败: ' . $e->getMessage()], 500);
            }
            break;


        // ========== 创建云端账号 ==========
        case 'create_account':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';
            $account_name = trim($_POST['account_name'] ?? '');
            $phone_id = trim($_POST['phone_id'] ?? '');
            $platform = trim($_POST['platform'] ?? '');
            $login_account = trim($_POST['login_account'] ?? '');
            $login_password = $_POST['login_password'] ?? '';
            $bind_phone = trim($_POST['bind_phone'] ?? '');
            $safe_link = trim($_POST['safe_link'] ?? '');
            $backup_email = trim($_POST['backup_email'] ?? '');
            $owner_id = (int)($_POST['owner_id'] ?? 0);
            $remark = trim($_POST['remark'] ?? '');
            $tags = $_POST['tags'] ?? '';
            $nickname = trim($_POST['nickname'] ?? '') ?: $account_name;

            // 记录所有收到的 POST 数据（密码除外）
            $debug_post = $_POST;
            if (isset($debug_post['password'])) $debug_post['password'] = '***';
            bwbrowser_log("create_account: 收到POST数据: " . json_encode($debug_post, JSON_UNESCAPED_UNICODE));
            bwbrowser_log("create_account: 解析后: account_name=$account_name, phone_id=$phone_id, login_account=$login_account, login_password=" . ($login_password ? '***' : 'NULL') . ", bind_phone=$bind_phone, safe_link=$safe_link, backup_email=$backup_email, owner_id=$owner_id, nickname=$nickname, platform=$platform");

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            if (empty($account_name)) {
                json_response(['success' => false, 'message' => '账号名称不能为空'], 400);
            }

            $stmt = $pdo->prepare("SELECT id FROM tiktok_accounts WHERE account_name = ? AND company_id = ? AND is_deleted = 0");
            $stmt->execute([$account_name, $user['company_id']]);
            if ($stmt->fetch()) {
                json_response(['success' => false, 'message' => '账号名称已存在'], 409);
            }

            $final_owner_id = $owner_id > 0 ? $owner_id : (int)$user['id'];
            $tags_json = !empty($tags) ? json_encode(is_array($tags) ? $tags : [$tags], JSON_UNESCAPED_UNICODE) : null;

            // 确保 backup_email 字段存在
            try {
                $pdo->exec("ALTER TABLE tiktok_accounts ADD COLUMN IF NOT EXISTS backup_email VARCHAR(255) DEFAULT NULL");
            } catch (Exception $e) {}

            try {
                $stmt = $pdo->prepare("INSERT INTO tiktok_accounts
                    (account_name, phone_id, account_nickname, login_account, login_password,
                     platform, bind_phone, safe_link, backup_email, owner_id, company_id,
                     tags_json, remark, status, pure_username, created_at, updated_at, is_deleted)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, NOW(), NOW(), 0)");
                $stmt->execute([
                    $account_name,
                    $phone_id ?: null,
                    $nickname,
                    $login_account ?: null,
                    $login_password ?: null,
                    $platform ?: null,
                    $bind_phone ?: null,
                    $safe_link ?: null,
                    $backup_email ?: null,
                    $final_owner_id,
                    (int)$user['company_id'],
                    $tags_json,
                    $remark ?: null,
                    $login_account ?: null,
                ]);
                $new_id = (int)$pdo->lastInsertId();
                bwbrowser_log("create_account: name=$account_name platform=$platform id=$new_id user={$user['username']}");

                // 同步插入 account_details 表
                try {
                    $ad_stmt = $pdo->prepare("INSERT INTO account_details
                        (account_id, device_id, login_account, login_password, bind_phone, safe_link, backup_email, created_at)
                        VALUES (?, ?, ?, ?, ?, ?, ?, NOW())");
                    $ad_stmt->execute([
                        $new_id,
                        $phone_id ?: null,
                        $login_account ?: null,
                        $login_password ?: null,
                        $bind_phone ?: null,
                        $safe_link ?: null,
                        $backup_email ?: null,
                    ]);
                    bwbrowser_log("create_account: account_details inserted for id=$new_id");
                } catch (Exception $ad_e) {
                    bwbrowser_log("create_account: account_details insert failed (non-fatal): " . $ad_e->getMessage(), 'WARN');
                }

                json_response(['success' => true, 'account_id' => $new_id, 'message' => '创建成功']);
            } catch (Exception $e) {
                bwbrowser_log("create_account failed: " . $e->getMessage(), 'ERROR');
                json_response(['success' => false, 'message' => '创建失败: ' . $e->getMessage()], 500);
            }
            break;

        // ========== 调试：查看表结构 ==========
        case 'debug_table':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';
            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }
            $tbl = $_POST['table'] ?? 'tiktok_accounts';
            $stmt = $pdo->prepare("DESCRIBE `$tbl`");
            $stmt->execute();
            $columns = $stmt->fetchAll();
            bwbrowser_log("debug_table: table=$tbl columns=" . count($columns));
            json_response(['success' => true, 'table' => $tbl, 'columns' => $columns]);
            break;

        // ========== 调试：查看单条账号原始数据 ==========
        case 'debug_account':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';
            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }
            $aid = (int)($_POST['account_id'] ?? 0);
            if ($aid <= 0) {
                json_response(['success' => false, 'message' => 'account_id required'], 400);
            }
            $stmt = $pdo->prepare("SELECT * FROM tiktok_accounts WHERE id = ?");
            $stmt->execute([$aid]);
            $row = $stmt->fetch();
            bwbrowser_log("debug_account: id=$aid found=" . ($row ? 'yes' : 'no'));
            json_response(['success' => true, 'account' => $row]);
            break;

        // ========== 公司列表（超级管理员专用） ==========
        case 'list_companies':
            $username = trim($_POST['username'] ?? '');
            $password = $_POST['password'] ?? '';

            $user = bwbrowser_get_user($pdo, $username, $password);
            if (!$user) {
                json_response(['success' => false, 'message' => '认证失败'], 401);
            }

            if (!bwbrowser_is_super_admin($user)) {
                json_response(['success' => false, 'message' => '仅超级管理员可查看公司列表'], 403);
            }

            $stmt = $pdo->query("SELECT id, company_name, company_code FROM companies WHERE status = 1 ORDER BY id ASC");
            $companies = $stmt->fetchAll();

            json_response([
                'success' => true,
                'companies' => array_map(function($c) {
                    return [
                        'id' => (int)$c['id'],
                        'name' => $c['company_name'],
                        'code' => $c['company_code'],
                    ];
                }, $companies),
            ]);
            break;
        // ========== 默认：未知 action ==========
        default:
            bwbrowser_log("未知 action: $ACTION", 'WARN');
            json_response([
                'success' => false,
                'message' => "未知的 action: $ACTION",
                'available_actions' => [
                    'login', 'get_permissions',
                    'list_proxies', 'sync_proxy', 'delete_proxy',
                    'list_envs', 'sync_env', 'delete_env',
                    'list_accounts', 'get_account_cookies', 'update_account_cookies',
                    'update_account_codes', 'update_account_env', 'update_account_info',
                    'log_video_download', 'create_account', 'get_account_detail', 'delete_account_cookies', 'list_companies',
                    'debug_table', 'debug_account',
                ],
            ], 400);
            break;
    }
} catch (Exception $e) {
    bwbrowser_log("EXCEPTION: " . $e->getMessage(), 'ERROR');
    json_response([
        'success' => false,
        'message' => '服务器内部错误: ' . $e->getMessage(),
    ], 500);
}

bwbrowser_log("=== 请求结束: action=$ACTION ===\n");
