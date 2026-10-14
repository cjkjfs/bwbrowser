<?php
/**
 * Simprint Accounts API
 * Returns tiktok/benchmark accounts for the logged-in user's company
 *
 * Actions:
 * - list:    Get account list (paginated)
 * - detail:  Get single account detail (with detail merged in main table + person info)
 * - summary: Get account summary stats
 * - search:  Search accounts by name/tag
 */

header('Content-Type: application/json; charset=utf-8');
header('Access-Control-Allow-Origin: *');
header('Access-Control-Allow-Methods: GET, POST, OPTIONS');
header('Access-Control-Allow-Headers: Content-Type, Authorization');

if ($_SERVER['REQUEST_METHOD'] === 'OPTIONS') {
    http_response_code(200);
    exit;
}

require_once __DIR__ . '/config.php';

// 确保 is_simprint 字段存在（兼容 MySQL 和 MariaDB）
$_col = $pdo->query("SHOW COLUMNS FROM tiktok_accounts LIKE 'is_simprint'");
if (!$_col || !$_col->fetch()) {
    $pdo->exec("ALTER TABLE tiktok_accounts ADD COLUMN is_simprint TINYINT(1) NOT NULL DEFAULT 0");
}

// 添加唯一索引防止同公司同平台同账号重复（不同公司互不干扰）
$_idx = $pdo->query("SELECT COUNT(*) FROM information_schema.STATISTICS WHERE table_schema = DATABASE() AND table_name = 'tiktok_accounts' AND index_name = 'uk_company_platform_user'");
if ($_idx && (int)$_idx->fetchColumn() === 0) {
    try { $pdo->exec("CREATE UNIQUE INDEX uk_company_platform_user ON tiktok_accounts (company_id, platform, pure_username)"); } catch (Exception $e) {}
}

$ACTION = $_POST['action'] ?? $_GET['action'] ?? '';
$input = json_decode(file_get_contents('php://input'), true);
if ($input) {
    foreach ($input as $k => $v) {
        if (!isset($_POST[$k])) $_POST[$k] = $v;
    }
}

function json_response($data, $code = 200) {
    // Always return 200 to prevent Nginx from intercepting 4xx/5xx and replacing JSON with HTML
    http_response_code(200);
    $data['_http_code'] = $code;
    echo json_encode($data, JSON_UNESCAPED_UNICODE);
    exit;
}

function get_user($pdo, $username, $password) {
    $stmt = $pdo->prepare("
        SELECT u.*, c.company_name, c.company_code
        FROM users u
        LEFT JOIN companies c ON u.company_id = c.id
        WHERE u.username = ? AND u.status = 'active'
    ");
    $stmt->execute([$username]);
    $user = $stmt->fetch(PDO::FETCH_ASSOC);
    if (!$user || !password_verify($password, $user['password'])) {
        return null;
    }
    unset($user['password']);
    unset($user['plain_password']);
    return $user;
}

/**
 * 组长：返回其「可管理小组」内所有在职用户 id 列表。
 * 小组判定：优先用 managed_group_ids(逗号分隔)；为空则回落自己的 group_id。
 * 若最终无组或组内无人，至少返回组长自己 id，保证其能看到自己账号。
 */
function leader_managed_owner_ids($pdo, $user) {
    $companyId = (int)$user['company_id'];
    $groupIds = [];
    $mgi = trim((string)($user['managed_group_ids'] ?? ''));
    if ($mgi !== '') {
        foreach (explode(',', $mgi) as $g) {
            $g = (int)trim($g);
            if ($g > 0) $groupIds[$g] = true;
        }
    }
    if (empty($groupIds)) {
        $g = (int)($user['group_id'] ?? 0);
        if ($g > 0) $groupIds[$g] = true;
    }
    if (empty($groupIds)) return [(int)$user['id']];

    $groupList = array_keys($groupIds);
    $ph = implode(',', array_fill(0, count($groupList), '?'));
    $stmt = $pdo->prepare("SELECT id FROM users WHERE company_id = ? AND group_id IN ($ph) AND status = 'active'");
    $stmt->execute(array_merge([$companyId], $groupList));
    $ids = array_values(array_filter(array_map('intval', $stmt->fetchAll(PDO::FETCH_COLUMN))));
    return empty($ids) ? [(int)$user['id']] : $ids;
}

// 到期守卫（温和版）：到期后拦截写/同步/新增类操作，读取与查看不受影响
$EXPIRED_BLOCK_ACTIONS = ['update_env_uuid', 'update_proxy', 'delete_cookie', 'update_account', 'upload_cookie', 'upload_fingerprint', 'delete', 'sync_account', 'create'];
if (in_array($ACTION, $EXPIRED_BLOCK_ACTIONS, true)) {
    $_u = get_user($pdo, trim($_POST['username'] ?? ''), $_POST['password'] ?? '');
    if ($_u) {
        $_svc = company_service_status($pdo, (int)$_u['company_id']);
        if (!$_svc['ok']) {
            json_response(['success' => false, 'message' => $_svc['message']], 400);
        }
    }
}

try {
    switch ($ACTION) {

        case 'list':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $page = max(1, (int)($_POST['page'] ?? 1));
            $page_size = min(200, max(1, (int)($_POST['page_size'] ?? 50)));
            $offset = ($page - 1) * $page_size;
            $platform = $_POST['platform'] ?? '';
            $status = $_POST['status'] ?? '';
            $keyword = trim($_POST['keyword'] ?? '');
            $owner_id = (int)($_POST['owner_id'] ?? 0);
            $account_type = $_POST['account_type'] ?? 'tiktok'; // tiktok or benchmark
            // 排序：白名单校验（followers / created_at），默认按创建时间倒序
            $sort_col = (($_POST['sort_by'] ?? '') === 'followers') ? 'followers' : 'created_at';
            $sort_dir = (strtoupper($_POST['sort_order'] ?? 'DESC') === 'ASC') ? 'ASC' : 'DESC';

            // 权限过滤：超级管理员/管理员看全公司；组长只看到其可管理小组成员账号；其余（含主管）默认只看自己
            $_is_super = !empty($user['is_super_admin']);
            $_is_manager = in_array(($user['role'] ?? ''), ['manager', 'admin', 'super_admin'], true);
            $_is_leader = (($user['role'] ?? '') === 'leader') && !$_is_super;
            $leaderGroupUserIds = [];
            if ($_is_leader) {
                $leaderGroupUserIds = leader_managed_owner_ids($pdo, $user);
                // 组长如果显式传了 owner_id，只有当该人属于其可管理小组时才按他过滤，否则退回按整组
                if ($owner_id > 0 && !in_array($owner_id, $leaderGroupUserIds, true)) {
                    $owner_id = 0;
                }
            }
            if (!$_is_manager && !$_is_super && !$_is_leader && $owner_id <= 0) {
                $owner_id = (int)$user['id'];
            }

            // Determine company filter
            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            if ($account_type === 'benchmark') {
                $table = 'benchmark_accounts';
                $name_field = 'username';
            } else {
                $table = 'tiktok_accounts';
                $name_field = 'account_name';
            }

            // Build WHERE
            $tblPrefix = $account_type === 'tiktok' ? 'tiktok_accounts' : $table;
            $where = ["{$tblPrefix}.company_id = ?", "{$tblPrefix}.is_deleted = 0"];
            $params = [$target_company_id];

            if ($platform) {
                $platforms = array_filter(array_map('trim', explode(',', $platform)));
                if (count($platforms) > 1) {
                    $placeholders = implode(',', array_fill(0, count($platforms), '?'));
                    $where[] = "{$tblPrefix}.platform IN ($placeholders)";
                    $params = array_merge($params, $platforms);
                } else {
                    $where[] = "{$tblPrefix}.platform = ?";
                    $params[] = $platform;
                }
            }
            if ($account_type === 'tiktok' && $status !== '') {
                $where[] = "{$tblPrefix}.status = ?";
                $params[] = (int)$status;
            }
            if ($owner_id > 0) {
                // 显式指定归属人：对组长已在上方校验过其属于可管理小组，此时只看该人
                $where[] = "{$tblPrefix}.owner_id = ?";
                $params[] = $owner_id;
            } elseif (!empty($leaderGroupUserIds)) {
                $ph = implode(',', array_fill(0, count($leaderGroupUserIds), '?'));
                $where[] = "{$tblPrefix}.owner_id IN ($ph)";
                $params = array_merge($params, $leaderGroupUserIds);
            }
            if ($keyword) {
                if ($account_type === 'tiktok') {
                    $where[] = "(tiktok_accounts.account_name LIKE ? OR tiktok_accounts.nickname LIKE ? OR tiktok_accounts.tags LIKE ? OR tiktok_accounts.pure_username LIKE ? OR tiktok_accounts.remark LIKE ? OR tiktok_accounts.proxy_node LIKE ? OR tiktok_accounts.device_id LIKE ? OR tiktok_accounts.login_account LIKE ? OR tiktok_accounts.bind_phone LIKE ? OR tiktok_accounts.safe_link LIKE ? OR u.real_name LIKE ? OR u.username LIKE ? OR sp.host LIKE ?)";
                    $kw = "%$keyword%";
                    $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw; $params[] = $kw;
                } else {
                    $where[] = "($name_field LIKE ? OR nickname LIKE ? OR tags LIKE ?)";
                    $kw = "%$keyword%";
                    $params[] = $kw; $params[] = $kw; $params[] = $kw;
                }
            }

            $whereClause = implode(' AND ', $where);

            // For tiktok with keyword, use LEFT JOIN to search across tables
            $joinClause = '';
            $useTiktokJoin = ($account_type === 'tiktok' && $keyword);
            if ($useTiktokJoin) {
                $joinClause = " LEFT JOIN users u ON u.id = tiktok_accounts.owner_id LEFT JOIN simprint_environments se ON tiktok_accounts.env_uuid = se.env_uuid LEFT JOIN simprint_proxies sp ON se.proxy_uuid = sp.proxy_uuid";
            }

            // Count
            if ($useTiktokJoin) {
                $countStmt = $pdo->prepare("SELECT COUNT(DISTINCT tiktok_accounts.id) FROM tiktok_accounts $joinClause WHERE $whereClause");
            } else {
                $countStmt = $pdo->prepare("SELECT COUNT(*) FROM $table WHERE $whereClause");
            }
            $countStmt->execute($params);
            $total = (int)$countStmt->fetchColumn();

            // Query
            if ($useTiktokJoin) {
                $selectFields = "tiktok_accounts.id, tiktok_accounts.account_name, tiktok_accounts.pure_username, tiktok_accounts.nickname, tiktok_accounts.platform, tiktok_accounts.account_uid, tiktok_accounts.tags, tiktok_accounts.category, tiktok_accounts.remark, tiktok_accounts.owner_id, tiktok_accounts.status, tiktok_accounts.followers, tiktok_accounts.likes, tiktok_accounts.video_count, tiktok_accounts.total_views, tiktok_accounts.visibility, tiktok_accounts.avatar, tiktok_accounts.verified, tiktok_accounts.cookie_updated_at, tiktok_accounts.last_logged_in_at, tiktok_accounts.proxy_node, tiktok_accounts.fingerprint_updated_at, tiktok_accounts.bind_person_id, tiktok_accounts.created_at, tiktok_accounts.env_uuid, tiktok_accounts.login_account, tiktok_accounts.login_password, tiktok_accounts.device_id, tiktok_accounts.safe_link, tiktok_accounts.bind_phone, sp.country AS proxy_country, sp.city AS proxy_city";
                $stmt = $pdo->prepare("SELECT DISTINCT $selectFields FROM tiktok_accounts $joinClause WHERE $whereClause ORDER BY tiktok_accounts.$sort_col $sort_dir LIMIT $page_size OFFSET $offset");
            } else {
                $selectFields = $account_type === 'tiktok'
                    ? "id, account_name, pure_username, nickname, platform, account_uid, tags, category, remark, owner_id, status, followers, likes, video_count, total_views, visibility, avatar, verified, cookie_updated_at, last_logged_in_at, proxy_node, fingerprint_updated_at, bind_person_id, created_at, env_uuid, login_account, login_password, device_id, safe_link, bind_phone"
                    : "id, username, nickname, platform, followers, following, likes, video_count, total_views, category, remark, visibility, status, owner_id, account_url, avatar, bio, tags, created_at";
                $stmt = $pdo->prepare("SELECT $selectFields FROM $table WHERE $whereClause ORDER BY $sort_col $sort_dir LIMIT $page_size OFFSET $offset");
            }
            $stmt->execute($params);
            $accounts = $stmt->fetchAll(PDO::FETCH_ASSOC);

            // Check password view permission
            $canViewPassword = (bool)$user['is_super_admin'] || in_array(($user['role'] ?? ''), ['manager', 'admin', 'super_admin'], true) || (bool)$user['allow_view_password'];

            // Get owner names
            $ownerIds = array_unique(array_filter(array_column($accounts, 'owner_id')));
            $ownerNames = [];
            if (!empty($ownerIds)) {
                $placeholders = implode(',', array_fill(0, count($ownerIds), '?'));
                $ownerStmt = $pdo->prepare("SELECT id, real_name, username FROM users WHERE id IN ($placeholders)");
                $ownerStmt->execute(array_values($ownerIds));
                foreach ($ownerStmt->fetchAll(PDO::FETCH_ASSOC) as $ou) {
                    $ownerNames[$ou['id']] = $ou['real_name'] ?: $ou['username'];
                }
            }

            // Get bind person names
            $personIds = array_unique(array_filter(array_column($accounts, 'bind_person_id')));
            $personNames = [];
            if (!empty($personIds)) {
                $placeholders = implode(',', array_fill(0, count($personIds), '?'));
                $personStmt = $pdo->prepare("SELECT id, firstname, lastname FROM persons WHERE id IN ($placeholders)");
                $personStmt->execute(array_values($personIds));
                foreach ($personStmt->fetchAll(PDO::FETCH_ASSOC) as $pu) {
                    $name = trim(($pu['firstname'] ?? '') . ' ' . ($pu['lastname'] ?? ''));
                    $personNames[$pu['id']] = $name ?: ('#' . $pu['id']);
                }
            }

            // Format
            $list = array_map(function($a) use ($ownerNames, $account_type, $personNames, $canViewPassword) {
                return [
                    'id' => (int)$a['id'],
                    'account_name' => $a['account_name'] ?? '',
                    'login_account' => $a['login_account'] ?? '',
                    'login_password' => $canViewPassword ? ($a['login_password'] ?? '') : '***',
                    'device_id' => $a['device_id'] ?? '',
                    'phone_id' => $a['device_id'] ?? '',
                    'safe_link' => $a['safe_link'] ?? '',
                    'bind_phone' => $a['bind_phone'] ?? '',
                    'pure_username' => $a['pure_username'] ?? '',
                    'nickname' => $a['nickname'] ?? '',
                    'platform' => $a['platform'] ?? '',
                    'tags' => $a['tags'] ?? '',
                    'category' => $a['category'] ?? '',
                    'remark' => $a['remark'] ?? '',
                    'owner_id' => (int)($a['owner_id'] ?? 0),
                    'owner_name' => $ownerNames[$a['owner_id'] ?? 0] ?? '',
                    'status' => isset($a['status']) ? (int)$a['status'] : 1,
                    'followers' => (int)($a['followers'] ?? 0),
                    'likes' => (int)($a['likes'] ?? 0),
                    'video_count' => (int)($a['video_count'] ?? 0),
                    'total_views' => (int)($a['total_views'] ?? 0),
                    'visibility' => $a['visibility'] ?? 'public',
                    'avatar' => $a['avatar'] ?? $a['avatar'] ?? '',
                    'verified' => isset($a['verified']) ? (bool)$a['verified'] : false,
                    'cookie_updated_at' => $a['cookie_updated_at'] ?? null,
                    'last_logged_in_at' => $a['last_logged_in_at'] ?? null,
                    'proxy_node' => $a['proxy_node'] ?? '',
                    'proxy_country' => $a['proxy_country'] ?? '',
                    'proxy_city' => $a['proxy_city'] ?? '',
                    'fingerprint_updated_at' => $a['fingerprint_updated_at'] ?? null,
                    'bind_person_id' => isset($a['bind_person_id']) ? (int)$a['bind_person_id'] : 0,
                    'bind_person_name' => $personNames[$a['bind_person_id'] ?? 0] ?? '',
                    'account_type' => $account_type,
                    'created_at' => $a['created_at'] ?? '',
                    'env_uuid' => $a['env_uuid'] ?? '',
                ];
            }, $accounts);

            json_response([
                'success' => true,
                'total' => $total,
                'page' => $page,
                'page_size' => $page_size,
                'accounts' => $list,
                'is_super_admin' => (bool)$user['is_super_admin'],
                'is_manager' => in_array(($user['role'] ?? ''), ['manager', 'admin', 'super_admin', 'leader'], true),
                'can_view_password' => $canViewPassword,
                'can_view_2fa' => in_array(($user['role'] ?? ''), ['manager', 'admin', 'super_admin', 'leader'], true) || (bool)($user['allow_2fa'] ?? 0),
                'can_view_sms' => in_array(($user['role'] ?? ''), ['manager', 'admin', 'super_admin', 'leader'], true) || (bool)($user['allow_sms_management'] ?? 0),
                'company_name' => $user['company_name'] ?? '',
            ]);
            break;

        case 'update_env_uuid':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $account_id = (int)($_POST['account_id'] ?? 0);
            $env_uuid = trim($_POST['env_uuid'] ?? '');
            if ($account_id <= 0) json_response(['success' => false, 'message' => '无效的账号ID'], 400);

            $stmt = $pdo->prepare("UPDATE tiktok_accounts SET env_uuid = ? WHERE id = ? AND company_id = ?");
            $stmt->execute([$env_uuid ?: null, $account_id, $user['company_id']]);
            json_response(['success' => true, 'message' => 'env_uuid updated']);
            break;

        case 'update_proxy':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $account_id = (int)($_POST['account_id'] ?? 0);
            $proxy_node = trim($_POST['proxy_node'] ?? '');
            if ($account_id <= 0) json_response(['success' => false, 'message' => '无效的账号ID'], 400);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }


            // 确保 proxy_node 列为 TEXT 类型（兼容长代理字符串）
            $_pn = $pdo->query("SHOW COLUMNS FROM tiktok_accounts LIKE 'proxy_node'");
            if ($_pn && ($_pn_row = $_pn->fetch(PDO::FETCH_ASSOC))) {
                if (stripos($_pn_row['Type'], 'text') === false) {
                    $pdo->exec("ALTER TABLE tiktok_accounts MODIFY COLUMN proxy_node TEXT");
                }
            }
            $stmt = $pdo->prepare("UPDATE tiktok_accounts SET proxy_node = ? WHERE id = ? AND company_id = ?");
            $stmt->execute([$proxy_node ?: null, $account_id, $target_company_id]);
            json_response(['success' => true, 'message' => '代理已更新']);
            break;

        case 'delete_cookie':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) json_response(['success' => false, 'message' => '无效的账号ID'], 400);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            // 删除 tiktok_accounts.cookie
            $stmt = $pdo->prepare("UPDATE tiktok_accounts SET cookie = NULL, cookie_updated_at = NULL WHERE id = ? AND company_id = ?");
            $stmt->execute([$account_id, $target_company_id]);

            // 同时删除 simprint_env_cookies 表中的记录（按 account_id 关联）
            $envStmt = $pdo->prepare("SELECT env_uuid FROM tiktok_accounts WHERE id = ? AND company_id = ?");
            $envStmt->execute([$account_id, $target_company_id]);
            $envRow = $envStmt->fetch(PDO::FETCH_ASSOC);
            if ($envRow && !empty($envRow['env_uuid'])) {
                $delStmt = $pdo->prepare("DELETE FROM simprint_env_cookies WHERE env_uuid = ? AND company_id = ?");
                $delStmt->execute([$envRow['env_uuid'], $target_company_id]);
            }

            json_response(['success' => true, 'message' => '服务器 Cookie 已删除']);
            break;

        case 'update_account':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            // 参照爆文库角色体系：manager = 公司管理员（经理）
            // 管理员（manager及以上）可修改全部字段
            // 主管(supervisor)及账号归属人可修改代理/备注/标签
            $isAdmin = (bool)$user['is_super_admin'] || in_array($user['role'] ?? '', ['manager', 'super_admin'], true);

            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) json_response(['success' => false, 'message' => '无效的账号ID'], 400);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            $checkStmt = $pdo->prepare("SELECT * FROM tiktok_accounts WHERE id = ? AND company_id = ?");
            $checkStmt->execute([$account_id, $target_company_id]);
            $existing = $checkStmt->fetch(PDO::FETCH_ASSOC);
            if (!$existing) {
                json_response(['success' => false, 'message' => '账号不存在或无权访问'], 404);
            }

            $isOwner = isset($existing['owner_id']) && $existing['owner_id'] == $user['id'];
            $canEdit = $isAdmin || $isOwner || in_array($user['role'] ?? '', ['supervisor'], true);
            if (!$canEdit) {
                json_response(['success' => false, 'message' => '无修改账号权限'], 403);
            }

            if ($isAdmin) {
                // 管理员：可以修改账号名称、昵称、平台等所有字段
                $new_name = trim($_POST['account_name'] ?? '') !== '' ? trim($_POST['account_name']) : $existing['account_name'];
                $new_platform = trim($_POST['platform'] ?? '') !== '' ? trim($_POST['platform']) : $existing['platform'];
                $new_pure_username = $existing['pure_username'];

                // 名称或平台变更时检查重复（排除自身）
                if ($new_name !== $existing['account_name'] || $new_platform !== $existing['platform']) {
                    $dupStmt = $pdo->prepare("SELECT id FROM tiktok_accounts WHERE company_id = ? AND platform = ? AND account_name = ? AND id != ? ORDER BY id ASC LIMIT 1");
                    $dupStmt->execute([$target_company_id, $new_platform, $new_name, $account_id]);
                    if ($dupStmt->fetch()) {
                        json_response(['success' => false, 'message' => '该账号名称在此平台已存在，请勿重复'], 400);
                    }
                    $dupStmt2 = $pdo->prepare("SELECT id FROM tiktok_accounts WHERE company_id = ? AND platform = ? AND pure_username = ? AND id != ? ORDER BY id ASC LIMIT 1");
                    $dupStmt2->execute([$target_company_id, $new_platform, $new_pure_username, $account_id]);
                    if ($dupStmt2->fetch()) {
                        json_response(['success' => false, 'message' => '该登录账号在此平台已存在，请勿重复'], 400);
                    }
                }

                $updates = "account_name = ?, nickname = ?, platform = ?, remark = ?, tags = ?";
                $vals = [$new_name, trim($_POST['nickname'] ?? ''), $new_platform, trim($_POST['remark'] ?? ''), trim($_POST['tags'] ?? '')];

                // 代理节点：仅在前端显式传 proxy_node 时才更新（编辑账号不传则保留原值，代理通过环境设置同步）
                if (array_key_exists('proxy_node', $_POST)) {
                    $updates .= ", proxy_node = ?";
                    $vals[] = trim($_POST['proxy_node']) ?: null;
                }

                // 归属人：显式传 owner_id 才更新（0 表示清除归属）
                if (array_key_exists('owner_id', $_POST)) {
                    $owner_id = (int)$_POST['owner_id'];
                    if ($owner_id > 0) {
                        $ownerStmt = $pdo->prepare("SELECT id FROM users WHERE id = ? AND company_id = ?");
                        $ownerStmt->execute([$owner_id, $target_company_id]);
                        if (!$ownerStmt->fetch()) {
                            json_response(['success' => false, 'message' => '归属人无效'], 400);
                        }
                        $updates .= ", owner_id = ?";
                        $vals[] = $owner_id;
                    } else {
                        $updates .= ", owner_id = NULL";
                    }
                }

                // 登录账号/密码/绑定手机/安全链接/手机编号：仅管理员可改（字段已合并到主表）
                $login_account = trim($_POST['login_account'] ?? '');
                $login_password = trim($_POST['login_password'] ?? '');
                $bind_phone = trim($_POST['bind_phone'] ?? '');
                $safe_link = trim($_POST['safe_link'] ?? '');
                $device_id = trim($_POST['device_id'] ?? '');
                $backup_email = trim($_POST['backup_email'] ?? '');
                if ($login_account !== '') { $updates .= ", login_account = ?"; $vals[] = $login_account; }
                if ($login_password !== '') { $updates .= ", login_password = ?"; $vals[] = $login_password; }
                if ($bind_phone !== '') { $updates .= ", bind_phone = ?"; $vals[] = $bind_phone; }
                if ($safe_link !== '') { $updates .= ", safe_link = ?"; $vals[] = $safe_link; }
                if ($device_id !== '') { $updates .= ", device_id = ?"; $vals[] = $device_id; }
                if ($backup_email !== '') { $updates .= ", backup_email = ?"; $vals[] = $backup_email; }
            } else {
                // 非管理员：只允许修改代理、备注、标签
                $updates = "remark = ?, tags = ?";
                $vals = [trim($_POST['remark'] ?? ''), trim($_POST['tags'] ?? '')];
                if (array_key_exists('proxy_node', $_POST)) {
                    $updates .= ", proxy_node = ?";
                    $vals[] = trim($_POST['proxy_node']) ?: null;
                }
            }

            $updates .= ", updated_at = NOW()";
            $vals[] = $account_id;

            $pdo->beginTransaction();
            try {
                $pdo->prepare("UPDATE tiktok_accounts SET $updates WHERE id = ?")->execute($vals);
                $pdo->commit();
            } catch (Exception $e) {
                $pdo->rollBack();
                json_response(['success' => false, 'message' => '修改账号失败: ' . $e->getMessage()], 500);
            }

            json_response(['success' => true, 'message' => '账号信息已更新']);
            break;

        case 'detail':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $account_id = (int)($_POST['account_id'] ?? 0);
            $account_type = $_POST['account_type'] ?? 'tiktok';
            if ($account_id <= 0) json_response(['success' => false, 'message' => '无效的账号ID'], 400);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            if ($account_type === 'benchmark') {
                $stmt = $pdo->prepare("SELECT * FROM benchmark_accounts WHERE id = ? AND company_id = ?");
                $stmt->execute([$account_id, $target_company_id]);
            } else {
                $stmt = $pdo->prepare("SELECT * FROM tiktok_accounts WHERE id = ? AND company_id = ?");
                $stmt->execute([$account_id, $target_company_id]);
            }

            $account = $stmt->fetch(PDO::FETCH_ASSOC);
            if (!$account) json_response(['success' => false, 'message' => '账号不存在或无权访问'], 404);

            // Get account details (merged into main table, only for tiktok)
            $detail = null;
            if ($account_type === 'tiktok') {
                $canViewPassword = (bool)$user['is_super_admin'] || in_array(($user['role'] ?? ''), ['manager', 'admin', 'super_admin'], true) || (bool)$user['allow_view_password'];
                $detail = [
                    'login_account' => $account['login_account'] ?? '',
                    'login_password' => $canViewPassword ? ($account['login_password'] ?? '') : '***',
                    'bind_email' => $account['bind_email'] ?? '',
                    'email_password' => $canViewPassword ? ($account['email_password'] ?? '') : '***',
                    'bind_phone' => $account['bind_phone'] ?? '',
                    'safe_link' => $account['safe_link'] ?? '',
                    'device_id' => $account['device_id'] ?? '',
                    'node_ip' => $account['node_ip'] ?? '',
                    'node_expire_date' => $account['node_expire_date'] ?? '',
                    'paypal_email' => $account['paypal_email'] ?? '',
                    'backup_email' => $account['backup_email'] ?? '',
                    'remark' => $account['remark'] ?? '',
                ];
                if (!$canViewPassword) {
                    $account['login_password'] = '***';
                }
            }

            // Get bound person info
            $person = null;
            if (!empty($account['bind_person_id'])) {
                $personStmt = $pdo->prepare("SELECT id, firstname, lastname, address, city, state, zip_code, phone, dob, dl_no, dl_state, remark FROM persons WHERE id = ? AND company_id = ? AND is_active = 1");
                $personStmt->execute([$account['bind_person_id'], $target_company_id]);
                $person = $personStmt->fetch(PDO::FETCH_ASSOC);
            }

            // Get owner name
            $ownerName = '';
            if (!empty($account['owner_id'])) {
                $ownerStmt = $pdo->prepare("SELECT real_name, username FROM users WHERE id = ?");
                $ownerStmt->execute([$account['owner_id']]);
                $ou = $ownerStmt->fetch(PDO::FETCH_ASSOC);
                if ($ou) $ownerName = $ou['real_name'] ?: $ou['username'];
            }

            json_response([
                'success' => true,
                'account' => [
                    'id' => (int)$account['id'],
                    'account_name' => $account['account_name'] ?? $account['username'] ?? '',
                    'pure_username' => $account['pure_username'] ?? '',
                    'nickname' => $account['nickname'] ?? '',
                    'platform' => $account['platform'] ?? '',
                    'tags' => $account['tags'] ?? '',
                    'category' => $account['category'] ?? '',
                    'remark' => $account['remark'] ?? '',
                    'status' => isset($account['status']) ? (int)$account['status'] : 1,
                    'followers' => (int)($account['followers'] ?? 0),
                    'likes' => (int)($account['likes'] ?? 0),
                    'video_count' => (int)($account['video_count'] ?? 0),
                    'total_views' => (int)($account['total_views'] ?? 0),
                    'visibility' => $account['visibility'] ?? 'public',
                    'avatar' => $account['avatar'] ?? $account['avatar'] ?? '',
                    'verified' => isset($account['verified']) ? (bool)$account['verified'] : false,
                    'owner_name' => $ownerName,
                    'owner_id' => isset($account['owner_id']) ? (int)$account['owner_id'] : 0,
                    'proxy_node' => $account['proxy_node'] ?? '',
                    'cookie' => !empty($account['cookie']) ? $account['cookie'] : '',
                    'cookie_updated_at' => $account['cookie_updated_at'] ?? null,
                    'last_logged_in_at' => $account['last_logged_in_at'] ?? null,
                    'fingerprint' => $account['fingerprint'] ?? '',
                    'fingerprint_updated_at' => $account['fingerprint_updated_at'] ?? null,
                    'bind_person_id' => isset($account['bind_person_id']) ? (int)$account['bind_person_id'] : 0,
                    'created_at' => $account['created_at'] ?? '',
                    'updated_at' => $account['updated_at'] ?? '',
                ],
                'detail' => $detail ? [
                    'login_account' => $detail['login_account'] ?? '',
                    'login_password' => $detail['login_password'] ?? '',
                    'bind_email' => $detail['bind_email'] ?? '',
                    'email_password' => $detail['email_password'] ?? '',
                    'bind_phone' => $detail['bind_phone'] ?? '',
                    'safe_link' => $detail['safe_link'] ?? '',
                    'device_id' => $detail['device_id'] ?? '',
                    'node_ip' => $detail['node_ip'] ?? '',
                    'node_expire_date' => $detail['node_expire_date'] ?? '',
                    'paypal_email' => $detail['paypal_email'] ?? '',
                    'backup_email' => $detail['backup_email'] ?? '',
                    'remark' => $detail['remark'] ?? '',
                ] : null,
                'person' => $person ? [
                    'id' => (int)$person['id'],
                    'firstname' => $person['firstname'] ?? '',
                    'lastname' => $person['lastname'] ?? '',
                    'address' => $person['address'] ?? '',
                    'city' => $person['city'] ?? '',
                    'state' => $person['state'] ?? '',
                    'zip_code' => $person['zip_code'] ?? '',
                    'phone' => $person['phone'] ?? '',
                    'dob' => $person['dob'] ?? '',
                    'dl_no' => $person['dl_no'] ?? '',
                    'dl_state' => $person['dl_state'] ?? '',
                    'remark' => $person['remark'] ?? '',
                ] : null,
            ]);
            break;

        case 'summary':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            // Build owner filter
            $owner_id = (int)($_POST['owner_id'] ?? 0);
            $ownerFilter = '';
            $ownerParams = [];
            $leaderGroupUserIds = [];
            if ((($user['role'] ?? '') === 'leader') && empty($user['is_super_admin'])) {
                $leaderGroupUserIds = leader_managed_owner_ids($pdo, $user);
                // 组长点选了某位成员时，仅当该成员属于其可管理小组才按他过滤，否则退回整组
                if ($owner_id > 0 && !in_array($owner_id, $leaderGroupUserIds, true)) $owner_id = 0;
            }
            if ($owner_id > 0) {
                $ownerFilter = ' AND owner_id = ?';
                $ownerParams = [$owner_id];
            } elseif (!empty($leaderGroupUserIds)) {
                if (count($leaderGroupUserIds) === 1) {
                    $ownerFilter = ' AND owner_id = ?';
                    $ownerParams = array_values($leaderGroupUserIds);
                } else {
                    $ownerFilter = ' AND owner_id IN (' . implode(',', array_fill(0, count($leaderGroupUserIds), '?')) . ')';
                    $ownerParams = array_values($leaderGroupUserIds);
                }
            }

            // TikTok accounts stats
            $ttStmt = $pdo->prepare("
                SELECT
                    COUNT(*) as total,
                    SUM(CASE WHEN status = 1 THEN 1 ELSE 0 END) as active,
                    SUM(CASE WHEN status = 0 THEN 1 ELSE 0 END) as inactive,
                    SUM(followers) as total_followers,
                    SUM(likes) as total_likes,
                    SUM(video_count) as total_videos
                FROM tiktok_accounts WHERE company_id = ?{$ownerFilter}
            ");
            $ttStmt->execute(array_merge([$target_company_id], $ownerParams));
            $tt = $ttStmt->fetch(PDO::FETCH_ASSOC);

            // Benchmark accounts stats
            $bmStmt = $pdo->prepare("SELECT COUNT(*) as total, SUM(followers) as total_followers FROM benchmark_accounts WHERE company_id = ? AND status = 1{$ownerFilter}");
            $bmStmt->execute(array_merge([$target_company_id], $ownerParams));
            $bm = $bmStmt->fetch(PDO::FETCH_ASSOC);

            // Platform breakdown
            $pfStmt = $pdo->prepare("SELECT platform, COUNT(*) as count, SUM(followers) as followers FROM tiktok_accounts WHERE company_id = ?{$ownerFilter} GROUP BY platform");
            $pfStmt->execute(array_merge([$target_company_id], $ownerParams));
            $platforms = $pfStmt->fetchAll(PDO::FETCH_ASSOC);

            json_response([
                'success' => true,
                'tiktok' => [
                    'total' => (int)$tt['total'],
                    'active' => (int)$tt['active'],
                    'inactive' => (int)$tt['inactive'],
                    'total_followers' => (int)$tt['total_followers'],
                    'total_likes' => (int)$tt['total_likes'],
                    'total_videos' => (int)$tt['total_videos'],
                ],
                'benchmark' => [
                    'total' => (int)$bm['total'],
                    'total_followers' => (int)$bm['total_followers'],
                ],
                'platforms' => array_map(function($p) {
                    return ['platform' => $p['platform'], 'count' => (int)$p['count'], 'followers' => (int)$p['followers']];
                }, $platforms),
                'company_name' => $user['company_name'] ?? '',
            ]);
            break;

        case 'companies':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);
            if (!$user['is_super_admin']) json_response(['success' => false, 'message' => '仅超级管理员可查看公司列表'], 403);

            $stmt = $pdo->query("SELECT c.id, c.company_name, c.company_code, c.status, COUNT(DISTINCT u.id) as user_count FROM companies c LEFT JOIN users u ON c.id=u.company_id AND u.status='active' WHERE c.status != 'deleted' GROUP BY c.id ORDER BY c.id");
            $companies = $stmt->fetchAll(PDO::FETCH_ASSOC);
            json_response(['success' => true, 'companies' => array_map(function($c) {
                return ['id'=>(int)$c['id'], 'company_name'=>$c['company_name'], 'company_code'=>$c['company_code'], 'status'=>$c['status'], 'user_count'=>(int)$c['user_count']];
            }, $companies)]);
            break;

        case 'upload_cookie':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $account_id = (int)($_POST['account_id'] ?? 0);
            $cookie = $_POST['cookie'] ?? '';
            if ($account_id <= 0) json_response(['success' => false, 'message' => '无效的账号ID'], 400);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            // Verify account exists and belongs to company
            $checkStmt = $pdo->prepare("SELECT id FROM tiktok_accounts WHERE id = ? AND company_id = ?");
            $checkStmt->execute([$account_id, $target_company_id]);
            if (!$checkStmt->fetch()) {
                json_response(['success' => false, 'message' => '账号不存在或无权访问'], 404);
            }

            // 空 cookie 不清空已有数据，直接返回提示
            if (empty($cookie) || $cookie === '[]' || $cookie === 'null' || $cookie === '{}') {
                json_response(['success' => false, 'message' => '未提取到Cookie，请检查浏览器中是否有对应平台的Cookie']);
            }

            $stmt = $pdo->prepare("UPDATE tiktok_accounts SET cookie = ?, cookie_updated_at = NOW(), last_logged_in_at = NOW(), is_simprint = 1 WHERE id = ? AND company_id = ?");
            $stmt->execute([$cookie, $account_id, $target_company_id]);
            json_response(['success' => true, 'message' => 'Cookie 上传成功', 'cookie_updated_at' => date('Y-m-d H:i:s')]);
            break;

        case 'upload_fingerprint':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $account_id = (int)($_POST['account_id'] ?? 0);
            $fingerprint = $_POST['fingerprint'] ?? '';
            if ($account_id <= 0) json_response(['success' => false, 'message' => '无效的账号ID'], 400);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            $checkStmt = $pdo->prepare("SELECT id FROM tiktok_accounts WHERE id = ? AND company_id = ?");
            $checkStmt->execute([$account_id, $target_company_id]);
            if (!$checkStmt->fetch()) {
                json_response(['success' => false, 'message' => '账号不存在或无权访问'], 404);
            }

            if (empty($fingerprint)) {
                $stmt = $pdo->prepare("UPDATE tiktok_accounts SET fingerprint = '', fingerprint_updated_at = NULL, is_simprint = 1 WHERE id = ?");
                $stmt->execute([$account_id]);
                json_response(['success' => true, 'message' => '指纹信息已清空', 'fingerprint_updated_at' => null]);
            }

            $stmt = $pdo->prepare("UPDATE tiktok_accounts SET fingerprint = ?, fingerprint_updated_at = NOW(), is_simprint = 1 WHERE id = ?");
            $stmt->execute([$fingerprint, $account_id]);
            json_response(['success' => true, 'message' => '指纹信息上传成功', 'fingerprint_updated_at' => date('Y-m-d H:i:s')]);
            break;

        case 'get_cookie':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) json_response(['success' => false, 'message' => '无效的账号ID'], 400);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            $stmt = $pdo->prepare("SELECT cookie, cookie_updated_at FROM tiktok_accounts WHERE id = ? AND company_id = ?");
            $stmt->execute([$account_id, $target_company_id]);
            $row = $stmt->fetch(PDO::FETCH_ASSOC);
            if (!$row) json_response(['success' => false, 'message' => '账号不存在或无权访问'], 404);

            json_response([
                'success' => true,
                'cookie' => $row['cookie'] ?? '',
                'cookie_updated_at' => $row['cookie_updated_at'] ?? null,
                'has_cookie' => !empty($row['cookie']),
            ]);
            break;

        case 'delete':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            // Only super admin or manager can delete
            $canDelete = (bool)$user['is_super_admin'] || $user['role'] === 'manager' || $user['role'] === 'admin' || $user['role'] === 'super_admin';
            if (!$canDelete) {
                json_response(['success' => false, 'message' => '无删除权限'], 403);
            }

            $account_id = (int)($_POST['account_id'] ?? 0);
            if ($account_id <= 0) json_response(['success' => false, 'message' => '无效的账号ID'], 400);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            // Verify account belongs to company
            $checkStmt = $pdo->prepare("SELECT id FROM tiktok_accounts WHERE id = ? AND company_id = ?");
            $checkStmt->execute([$account_id, $target_company_id]);
            if (!$checkStmt->fetch()) {
                json_response(['success' => false, 'message' => '账号不存在或无权访问'], 404);
            }

            // Delete the account (detail fields merged into main table)
            $pdo->prepare("DELETE FROM tiktok_accounts WHERE id = ?")->execute([$account_id]);

            json_response(['success' => true, 'message' => '账号已删除']);
            break;

        case 'log_launch':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $account_id = (int)($_POST['account_id'] ?? 0);
            $account_name = trim($_POST['account_name'] ?? '');
            $platform = trim($_POST['platform'] ?? '');
            $account_type = trim($_POST['account_type'] ?? 'account');
            $homepage_url = trim($_POST['homepage_url'] ?? '');
            $env_uuid = trim($_POST['env_uuid'] ?? '');
            $proxy_node = trim($_POST['proxy_node'] ?? '');
            $launch_status = trim($_POST['launch_status'] ?? 'success');
            $fail_reason = trim($_POST['fail_reason'] ?? '');

            // Get IP address
            $ip_address = $_SERVER['HTTP_X_FORWARDED_FOR'] ?? $_SERVER['REMOTE_ADDR'] ?? 'unknown';
            if (strpos($ip_address, ',') !== false) {
                $ip_address = explode(',', $ip_address)[0];
            }

            $stmt = $pdo->prepare("
                INSERT INTO browser_launch_logs 
                (user_id, user_name, user_real_name, company_id, account_id, account_name, platform, account_type, homepage_url, env_uuid, proxy_node, launch_status, fail_reason, ip_address, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NOW())
            ");
            $stmt->execute([
                $user['id'],
                $user['username'],
                $user['real_name'],
                $user['company_id'],
                $account_id,
                $account_name,
                $platform,
                $account_type,
                $homepage_url,
                $env_uuid,
                $proxy_node,
                $launch_status,
                $fail_reason,
                $ip_address
            ]);

            json_response(['success' => true, 'message' => '日志已记录', 'log_id' => $pdo->lastInsertId()]);
            break;

        case 'sync_account':
            // 指纹浏览器同步账号到服务器
            // UPSERT 逻辑：同公司同平台同用户名 → 更新（含 is_simprint=1）；否则新建
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $account_name = trim($_POST['account_name'] ?? '');
            $pure_username = trim($_POST['pure_username'] ?? '');
            $platform = trim($_POST['platform'] ?? '');
            $nickname = trim($_POST['nickname'] ?? '');
            $tags = trim($_POST['tags'] ?? '');
            $category = trim($_POST['category'] ?? '');
            $remark = trim($_POST['remark'] ?? '');
            $account_uid = trim($_POST['account_uid'] ?? '');
            $proxy_node = trim($_POST['proxy_node'] ?? '');

            if (empty($account_name) || empty($platform)) {
                json_response(['success' => false, 'message' => '账号名和平台不能为空'], 400);
            }
            // pure_username 为空时用 account_name 代替，确保唯一索引生效
            if (empty($pure_username)) $pure_username = $account_name;

            $target_company_id = (int)$user['company_id'];

            // 先查询是否已存在（同公司同平台同用户名），优先返回最早的记录
            $findStmt = $pdo->prepare("SELECT id FROM tiktok_accounts WHERE company_id = ? AND platform = ? AND pure_username = ? ORDER BY id ASC LIMIT 1");
            $findStmt->execute([$target_company_id, $platform, $pure_username]);
            $existing = $findStmt->fetch(PDO::FETCH_ASSOC);

            // 如果按 pure_username 没找到，再按 account_name 查找（防止同一账号因用户名不同而重复创建）
            if (!$existing) {
                $findStmt2 = $pdo->prepare("SELECT id FROM tiktok_accounts WHERE company_id = ? AND platform = ? AND account_name = ? ORDER BY id ASC LIMIT 1");
                $findStmt2->execute([$target_company_id, $platform, $account_name]);
                $existing = $findStmt2->fetch(PDO::FETCH_ASSOC);
            }

            if ($existing) {
                // 更新已有记录，标记为指纹浏览器账号（不更新 remark，保留云端备注）
                $stmt = $pdo->prepare("UPDATE tiktok_accounts SET
                    account_name = ?, nickname = ?, tags = ?, category = ?, account_uid = ?, proxy_node = ?,
                    is_simprint = 1, last_logged_in_at = NOW()
                    WHERE id = ?");
                $stmt->execute([$account_name, $nickname, $tags, $category, $account_uid, $proxy_node, $existing['id']]);
                $account_id = (int)$existing['id'];
                json_response(['success' => true, 'message' => '账号已更新', 'account_id' => $account_id, 'is_new' => false]);
            } else {
                // 账号只在云端建立，不再从客户端创建
                json_response(['success' => false, 'message' => '账号不存在，请在云端管理页面创建', 'is_new' => false]);
            }
            break;

        case 'list_users':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            // 组长只看其可管理小组成员
            $leaderGroupUserIds = [];
            if ((($user['role'] ?? '') === 'leader') && empty($user['is_super_admin'])) {
                $leaderGroupUserIds = leader_managed_owner_ids($pdo, $user);
            }
            if (!empty($leaderGroupUserIds)) {
                $ph = implode(',', array_fill(0, count($leaderGroupUserIds), '?'));
                $stmt = $pdo->prepare("SELECT id, username, real_name, role, status, sector FROM users WHERE company_id = ? AND status = 'active' AND id IN ($ph) ORDER BY id ASC");
                $stmt->execute(array_merge([$target_company_id], $leaderGroupUserIds));
            } else {
                $stmt = $pdo->prepare("SELECT id, username, real_name, role, status, sector FROM users WHERE company_id = ? AND status = 'active' ORDER BY id ASC");
                $stmt->execute([$target_company_id]);
            }
            $all_users = $stmt->fetchAll(PDO::FETCH_ASSOC);

            // 查询今日请假状态
            $today = date('Y-m-d');
            $user_ids = array_map(function($u) { return (int)$u['id']; }, $all_users);
            $leave_map = [];
            if (!empty($user_ids)) {
                $placeholders = implode(',', array_fill(0, count($user_ids), '?'));
                $stmt2 = $pdo->prepare("SELECT user_id, today_data FROM checkin_data_cache WHERE user_id IN ($placeholders) AND data_date = ?");
                $params2 = array_merge($user_ids, [$today]);
                $stmt2->execute($params2);
                foreach ($stmt2->fetchAll(PDO::FETCH_ASSOC) as $row) {
                    $td = json_decode($row['today_data'] ?? '{}', true) ?: [];
                    if (isset($td['leave_status'])) {
                        $leave_map[(int)$row['user_id']] = $td['leave_status'];
                    } else {
                        $leave_map[(int)$row['user_id']] = !empty($td['checked_in']) ? 'normal' : 'on_leave';
                    }
                }
            }

            $users = array_map(function($u) use ($leave_map) {
                return [
                    'id' => (int)$u['id'],
                    'username' => $u['username'] ?? '',
                    'real_name' => $u['real_name'] ?? '',
                    'role' => $u['role'] ?? 'member',
                    'status' => $u['status'] ?? 'active',
                    'sector' => $u['sector'] ?? '',
                    'leave_status' => $leave_map[(int)$u['id']] ?? 'unknown',
                ];
            }, $all_users);
            json_response(['success' => true, 'users' => $users]);
            break;

        case 'create':
            $user = get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');
            if (!$user) json_response(['success' => false, 'message' => '认证失败'], 401);

            // 仅管理员及以上可添加账号
            $canCreate = (bool)$user['is_super_admin'] || in_array($user['role'] ?? '', ['manager', 'supervisor', 'admin', 'super_admin'], true);
            if (!$canCreate) {
                json_response(['success' => false, 'message' => '无添加账号权限'], 403);
            }

            $account_name = trim($_POST['account_name'] ?? '');
            $login_account = trim($_POST['login_account'] ?? '');
            $login_password = trim($_POST['login_password'] ?? '');
            $platform = trim($_POST['platform'] ?? '');
            $owner_id = (int)($_POST['owner_id'] ?? 0);
            $proxy_node = trim($_POST['proxy_node'] ?? '');
            $remark = trim($_POST['remark'] ?? '');
            $nickname = trim($_POST['nickname'] ?? '');
            $bind_phone = trim($_POST['bind_phone'] ?? '');
            $safe_link = trim($_POST['safe_link'] ?? '');
            $device_id = trim($_POST['device_id'] ?? '');
            $tags = trim($_POST['tags'] ?? '');
            $backup_email = trim($_POST['backup_email'] ?? '');

            if (empty($account_name) || empty($platform)) {
                json_response(['success' => false, 'message' => '账号名称和平台不能为空'], 400);
            }

            $target_company_id = $user['company_id'];
            if ($user['is_super_admin'] && !empty($_POST['company_id'])) {
                $target_company_id = (int)$_POST['company_id'];
            }

            $pure_username = $login_account !== '' ? $login_account : $account_name;

            // 检查同公司同平台同用户名是否已存在
            $dupStmt = $pdo->prepare("SELECT id FROM tiktok_accounts WHERE company_id = ? AND platform = ? AND (pure_username = ? OR account_name = ?) ORDER BY id ASC LIMIT 1");
            $dupStmt->execute([$target_company_id, $platform, $pure_username, $account_name]);
            if ($dupStmt->fetch()) {
                json_response(['success' => false, 'message' => '该账号在此平台已存在，请勿重复添加'], 400);
            }

            // 校验归属人是否属于当前公司
            if ($owner_id > 0) {
                $ownerStmt = $pdo->prepare("SELECT id FROM users WHERE id = ? AND company_id = ?");
                $ownerStmt->execute([$owner_id, $target_company_id]);
                if (!$ownerStmt->fetch()) {
                    json_response(['success' => false, 'message' => '归属人无效'], 400);
                }
            } else {
                $owner_id = null;
            }

            // 套餐限制：校验公司自有账号数量上限
            $limitCheck = check_company_account_limit($pdo, $target_company_id);
            if (!$limitCheck['ok']) {
                json_response(['success' => false, 'message' => $limitCheck['message']], 400);
            }

            $pdo->beginTransaction();
            try {
                $stmt = $pdo->prepare("INSERT INTO tiktok_accounts (company_id, account_name, pure_username, nickname, platform, remark, owner_id, proxy_node, status, is_simprint, tags, login_account, login_password, bind_phone, safe_link, device_id, backup_email, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, 1, ?, ?, ?, ?, ?, ?, ?, NOW(), NOW())");
                $stmt->execute([$target_company_id, $account_name, $pure_username, $nickname, $platform, $remark, $owner_id, $proxy_node ?: null, $tags, $login_account ?: null, $login_password ?: null, $bind_phone ?: null, $safe_link ?: null, $device_id ?: null, $backup_email ?: null]);
                $account_id = (int)$pdo->lastInsertId();
                $pdo->commit();
            } catch (Exception $e) {
                $pdo->rollBack();
                json_response(['success' => false, 'message' => '创建账号失败: ' . $e->getMessage()], 500);
            }

            json_response(['success' => true, 'message' => '账号已创建', 'account_id' => $account_id]);
            break;

        default:
            json_response(['success' => false, 'message' => '未知操作: ' . $ACTION], 400);
    }
} catch (PDOException $e) {
    json_response(['success' => false, 'message' => '数据库错误: ' . $e->getMessage()], 500);
} catch (Exception $e) {
    json_response(['success' => false, 'message' => '服务器错误: ' . $e->getMessage()], 500);
}
