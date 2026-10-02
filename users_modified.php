<?php

// 强制开启错误日志
error_reporting(E_ALL);
ini_set('display_errors', 1);
ini_set('log_errors', 1);
ini_set('error_log', __DIR__ . '/logs/users_error.log');

// 记录开始时间
$logDir = __DIR__ . '/logs';
if (!is_dir($logDir)) { @mkdir($logDir, 0755, true); }
file_put_contents($logDir . '/users_error.log', date('Y-m-d H:i:s') . " - 脚本开始执行\n", FILE_APPEND);


require_once 'config.php';
require_once 'company_config.php';
if (!function_exists('log_operation')) {
    function log_operation($pdo, $user_id, $username, $real_name, $action, $target_type, $target_id, $target_name, $old_value, $new_value, $company_id = 0) {
        return false;
    }
}
$system_logo = get_system_logo($pdo);

// 模块权限清单：全局定义，API 与 Web 后台共用（依赖 permission_profile_fields() 等 global 取用）
$module_permissions = [
    'allow_video_download' => '视频下载',
    'allow_data_dashboard' => '数据看板',
    'allow_data_submit' => '数据上报',
    'allow_baowenku' => '爆文库',
    'allow_my_payslip' => '我的工资条',
    'allow_user_management' => '用户管理',
    'allow_data_report' => '数据报表',
    'allow_account_detail' => '账号详情',
    'allow_account_revenue' => '账号收益',
    'allow_salary_management' => '工资管理',
    'allow_finance' => '财务收支',
    'allow_operation_log' => '操作日志',
    'allow_wish_wall' => '许愿墙',
    'allow_memo' => '备忘录',
    'allow_sms_management' => '短信管理',
    'allow_2fa' => '安全链接(2FA)',
    'allow_profile' => '个人中心',
    'allow_company_settings' => '公司设置',
    'allow_remote_desktop' => '远程桌面',
    'allow_remote_management' => '远程管理'
];

// ============================================================
// JSON API 辅助函数（顶层定义，API 和 Web UI 均可使用）
// ============================================================
function json_response_users_api($data, $code = 200) {
    http_response_code(200);
    $data['_http_code'] = $code;
    echo json_encode($data, JSON_UNESCAPED_UNICODE);
    exit;
}

function api_get_user($pdo, $username, $password) {
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

function api_role_label($role) {
    $map = ['member' => '组员', 'leader' => '组长', 'supervisor' => '主管', 'hr' => '人事', 'manager' => '经理', 'admin' => '管理员', 'super_admin' => '超级管理员'];
    return $map[$role] ?? $role;
}

function api_can_manage($user) {
    return (bool)$user['is_super_admin'] || in_array($user['role'] ?? '', ['manager', 'supervisor', 'admin', 'super_admin'], true);
}

/**
 * 操作人能否授予该权限：超管可授任意权限；否则仅能授自己当前已持有的权限
 */
function api_can_grant_permission($grantor, $field) {
    if ((int)($grantor['is_super_admin'] ?? 0) === 1) return true;
    return (int)($grantor[$field] ?? 0) === 1;
}

function api_permission_label($field) {
    $map = permission_profile_fields();
    return $map[$field] ?? $field;
}

function api_user_owns_profile_perms($grantor, $flat) {
    foreach ($flat as $col => $v) {
        if ($v === 1 && !api_can_grant_permission($grantor, $col)) return false;
    }
    return true;
}

function scope_permitted_grants($grantor, $map) {
    if ((int)($grantor['is_super_admin'] ?? 0) === 1) return $map;
    $out = [];
    foreach ($map as $col => $v) {
        $out[$col] = ($v === 1 && (int)($grantor[$col] ?? 0) === 1) ? 1 : 0;
    }
    return $out;
}

// ============================================================
// JSON API 模式（供客户端 app 调用，POST + action 参数）
// 两种逻辑：
//   1. API 调用：传入 username + password 认证 → 返回 JSON 并 exit
//   2. 本地表单：Web UI 的 action=add（无 new_username）→ 落到下面 Web UI
// ============================================================
$api_actions = ['me','get_bwbrowser_cookies','update_bwbrowser_cookies','get_bwbrowser_bookmarks','update_bwbrowser_bookmarks','list','toggle_permission','roles','add','update','toggle_status','delete','permission_profiles','permission_profile_save','permission_profile_delete','permission_profile_apply'];
if ($_SERVER['REQUEST_METHOD'] === 'POST' && !empty($_POST['action']) && in_array($_POST['action'], $api_actions, true)) {
    $ACTION = $_POST['action'];
    $input = json_decode(file_get_contents('php://input'), true);
    if ($input) {
        foreach ($input as $k => $v) {
            if (!isset($_POST[$k])) $_POST[$k] = $v;
        }
    }

    // 尝试 API 认证（username + password）
    $api_user = api_get_user($pdo, $_POST['username'] ?? '', $_POST['password'] ?? '');

    // 认证失败时：如果是 Web UI 的 add 表单（无 new_username），落到 Web UI 处理
    if (!$api_user && $ACTION === 'add' && !isset($_POST['new_username'])) {
        // 本地表单提交，不走 API，继续执行下面的 Web 页面代码
    } else {
        // API 请求：认证失败返回 401，认证成功处理 API 逻辑
        header('Content-Type: application/json; charset=utf-8');
        header('Access-Control-Allow-Origin: *');
        header('Access-Control-Allow-Methods: GET, POST, OPTIONS');
        header('Access-Control-Allow-Headers: Content-Type, Authorization');

        $api_module_permissions = [
            'allow_video_download' => '视频下载',
            'allow_data_dashboard' => '数据看板',
            'allow_data_submit' => '数据上报',
            'allow_baowenku' => '爆文库',
            'allow_my_payslip' => '我的工资条',
            'allow_user_management' => '用户管理',
            'allow_data_report' => '数据报表',
            'allow_account_detail' => '账号详情',
            'allow_account_revenue' => '账号收益',
            'allow_salary_management' => '工资管理',
            'allow_finance' => '财务收支',
            'allow_operation_log' => '操作日志',
            'allow_wish_wall' => '许愿墙',
            'allow_memo' => '备忘录',
            'allow_sms_management' => '短信管理',
            'allow_2fa' => '安全链接(2FA)',
            'allow_profile' => '个人中心',
            'allow_company_settings' => '公司设置',
            'allow_remote_desktop' => '远程桌面',
            'allow_remote_management' => '远程管理',
            'allow_view_revenue' => '查看收益',
            'allow_view_password' => '查看密码',
            'allow_manage_users' => '管理用户',
        ];

        try {
            ensure_user_permission_columns($pdo);

            if (!$api_user) json_response_users_api(['success' => false, 'message' => '认证失败'], 401);

            $api_company_id = (int)$api_user['company_id'];
            if ($api_user['is_super_admin'] && !empty($_POST['company_id'])) {
                $api_company_id = (int)$_POST['company_id'];
            }

            // ===== me 接口：获取当前用户信息及权限（所有登录用户可访问）=====
            if ($ACTION === 'me') {
                // 公司套餐信息
                $me_company_id = (int)$api_user['company_id'];
                $me_plan = get_company_plan($pdo, $me_company_id);
                $me_plan_limit = company_account_limit($pdo, $me_company_id);
                $me_plan_count = count_company_accounts($pdo, $me_company_id);
                $me_billing = check_api_billing_status($pdo, $me_company_id);
                $me_company_row = null;
                if ($me_company_id > 0) {
                    $stmt = $pdo->prepare("SELECT plan, expire_date FROM companies WHERE id = ?");
                    $stmt->execute([$me_company_id]);
                    $me_company_row = $stmt->fetch(PDO::FETCH_ASSOC);
                }

                $perms = [];
                foreach ($api_module_permissions as $col => $lbl) {
                    $perms[$col] = (int)($api_user[$col] ?? 0) === 1;
                }
                $perms['allow_view_revenue'] =
                    (bool)$api_user['is_super_admin'] || in_array($api_user['role'] ?? '', ['manager', 'admin', 'super_admin']) ||
                    ((int)($api_user['allow_view_revenue'] ?? 0) === 1);
                $perms['allow_view_password'] =
                    (bool)$api_user['is_super_admin'] || in_array($api_user['role'] ?? '', ['manager', 'admin', 'super_admin']) ||
                    ((int)($api_user['allow_view_password'] ?? 0) === 1);
                $perms['allow_manage_users'] =
                    (bool)$api_user['is_super_admin'] || in_array($api_user['role'] ?? '', ['manager', 'admin', 'super_admin']) ||
                    ((int)($api_user['allow_manage_users'] ?? 0) === 1);

                json_response_users_api([
                    'success' => true,
                    'user' => [
                        'id' => (int)$api_user['id'],
                        'username' => $api_user['username'] ?? '',
                        'real_name' => $api_user['real_name'] ?? '',
                        'role' => $api_user['role'] ?? 'member',
                        'role_label' => api_role_label($api_user['role'] ?? 'member'),
                        'is_super_admin' => (bool)$api_user['is_super_admin'],
                        'company_id' => (int)$api_user['company_id'],
                        'permissions' => $perms,
                        'allow_sms_management' => $perms['allow_sms_management'] ?? false,
                        // 公司信息 + 套餐信息（账号中心展示用）
                        'company_name' => $api_user['company_name'] ?? '',
                        'company_plan' => $me_plan,
                        'company_plan_label' => company_plan_label($me_plan),
                        'company_account_limit' => $me_plan_limit,
                        'company_account_count' => $me_plan_count,
                        'company_expire_date' => $me_company_row['expire_date'] ?? null,
                        'company_balance' => $me_billing['balance'],
                        'company_balance_blocked' => $me_billing['blocked'],
                    ],
                ]);
            }

            // ===== get_bwbrowser_cookies 接口：获取当前用户的 BW Browser cookies（所有登录用户可访问）=====
            if ($ACTION === 'get_bwbrowser_cookies') {
                $stmt = $pdo->prepare("SELECT bwbrowser_cookies FROM users WHERE id = ? AND company_id = ?");
                $stmt->execute([(int)$api_user['id'], $api_company_id]);
                $row = $stmt->fetch(PDO::FETCH_ASSOC);
                json_response_users_api([
                    'success' => true,
                    'cookies' => $row['bwbrowser_cookies'] ?? null,
                ]);
            }

            // ===== update_bwbrowser_cookies 接口：更新当前用户的 BW Browser cookies（所有登录用户可访问）=====
            if ($ACTION === 'update_bwbrowser_cookies') {
                $cookies = isset($_POST['cookies']) ? (string)$_POST['cookies'] : null;
                $stmt = $pdo->prepare("UPDATE users SET bwbrowser_cookies = ? WHERE id = ? AND company_id = ?");
                $stmt->execute([$cookies, (int)$api_user['id'], $api_company_id]);
                json_response_users_api(['success' => true]);
            }


            // ===== get_bwbrowser_bookmarks 接口：获取当前用户的 BW Browser 书签（所有登录用户可访问）=====
            if ($ACTION === 'get_bwbrowser_bookmarks') {
                // 自动确保字段存在
                $check_col = $pdo->query("SHOW COLUMNS FROM users LIKE 'bwbrowser_bookmarks'");
                if (!$check_col->fetch()) {
                    $pdo->exec("ALTER TABLE users ADD COLUMN bwbrowser_bookmarks TEXT NULL COMMENT '爆文库书签同步'");
                }
                $stmt = $pdo->prepare("SELECT bwbrowser_bookmarks FROM users WHERE id = ? AND company_id = ?");
                $stmt->execute([(int)$api_user['id'], $api_company_id]);
                $row = $stmt->fetch(PDO::FETCH_ASSOC);
                json_response_users_api([
                    'success' => true,
                    'bookmarks' => $row['bwbrowser_bookmarks'] ?? null,
                ]);
            }

            // ===== update_bwbrowser_bookmarks 接口：更新当前用户的 BW Browser 书签（所有登录用户可访问）=====
            if ($ACTION === 'update_bwbrowser_bookmarks') {
                $bookmarks = isset($_POST['bookmarks']) ? (string)$_POST['bookmarks'] : null;
                // 自动确保字段存在
                $check_col = $pdo->query("SHOW COLUMNS FROM users LIKE 'bwbrowser_bookmarks'");
                if (!$check_col->fetch()) {
                    $pdo->exec("ALTER TABLE users ADD COLUMN bwbrowser_bookmarks TEXT NULL COMMENT '爆文库书签同步'");
                }
                $stmt = $pdo->prepare("UPDATE users SET bwbrowser_bookmarks = ? WHERE id = ? AND company_id = ?");
                $stmt->execute([$bookmarks, (int)$api_user['id'], $api_company_id]);
                json_response_users_api(['success' => true]);
            }

            // ===== 以下接口需要用户管理权限 =====
            if (!api_can_manage($api_user)) {
                json_response_users_api(['success' => false, 'message' => '无用户管理权限'], 403);
            }

            switch ($ACTION) {
                case 'list':
                    $stmt = $pdo->prepare("SELECT * FROM users WHERE company_id = ? ORDER BY is_super_admin DESC, FIELD(role, 'manager', 'hr', 'supervisor', 'leader', 'member'), id ASC");
                    $stmt->execute([$api_company_id]);
                    $users = array_map(function($u) use ($api_module_permissions) {
                        // 返回与权限组相同的全字段权限表，便于按权限值回退匹配权限组
                        $permissions = [];
                        foreach (permission_profile_fields() as $column => $label) {
                            $permissions[$column] = (int)($u[$column] ?? 0) === 1;
                        }
                        return [
                            'id' => (int)$u['id'],
                            'username' => $u['username'] ?? '',
                            'real_name' => $u['real_name'] ?? '',
                            'role' => $u['role'] ?? 'member',
                            'role_label' => api_role_label($u['role'] ?? 'member'),
                            'status' => $u['status'] ?? 'active',
                            'is_super_admin' => (bool)$u['is_super_admin'],
                            'phone' => $u['phone'] ?? '',
                            'email' => $u['email'] ?? '',
                            'created_at' => $u['created_at'] ?? '',
                            'last_login_at' => $u['last_login_at'] ?? '',
                            'permission_profile_id' => isset($u['permission_profile_id']) ? (int)$u['permission_profile_id'] : null,
                            'permissions' => $permissions,
                        ];
                    }, $stmt->fetchAll(PDO::FETCH_ASSOC));
                    json_response_users_api(['success' => true, 'users' => $users, 'is_super_admin' => (bool)$api_user['is_super_admin'], 'permission_labels' => $api_module_permissions]);
                    break;

                case 'toggle_permission':
                    $user_id = (int)($_POST['user_id'] ?? 0);
                    $permission = trim($_POST['permission'] ?? '');
                    $value = (int)($_POST['value'] ?? 0);

                    $all_perm_keys = array_merge(
                        array_keys($api_module_permissions),
                        ['allow_view_revenue', 'allow_view_password', 'allow_manage_users']
                    );

                    if ($user_id <= 0) json_response_users_api(['success' => false, 'message' => '无效的用户ID'], 400);
                    if (!in_array($permission, $all_perm_keys, true)) {
                        json_response_users_api(['success' => false, 'message' => '无效的权限字段'], 400);
                    }
                    if ($user_id === (int)$api_user['id'] && !$api_user['is_super_admin']) {
                        json_response_users_api(['success' => false, 'message' => '不能修改自己的权限'], 403);
                    }

                    $stmt = $pdo->prepare("SELECT is_super_admin FROM users WHERE id = ? AND company_id = ?");
                    $stmt->execute([$user_id, $api_company_id]);
                    $target = $stmt->fetch(PDO::FETCH_ASSOC);
                    if (!$target) {
                        json_response_users_api(['success' => false, 'message' => '用户不存在'], 404);
                    }
                    if ((int)$target['is_super_admin'] && !$api_user['is_super_admin']) {
                        json_response_users_api(['success' => false, 'message' => '不能修改超级管理员权限'], 403);
                    }

                    if ($value && !api_can_grant_permission($api_user, $permission)) {
                        json_response_users_api(['success' => false, 'message' => '你尚未拥有「' . api_permission_label($permission) . '」权限，不能为他人开启'], 403);
                    }

                    $pdo->prepare("UPDATE users SET {$permission} = ? WHERE id = ? AND company_id = ?")
                        ->execute([$value ? 1 : 0, $user_id, $api_company_id]);

                    $all_labels = array_merge($api_module_permissions, [
                        'allow_view_revenue' => '查看收益',
                        'allow_view_password' => '查看密码',
                        'allow_manage_users' => '管理用户',
                    ]);
                    $label = $all_labels[$permission] ?? $permission;
                    json_response_users_api(['success' => true, 'message' => $label . '已' . ($value ? '开启' : '关闭')]);
                    break;

                case 'roles':
                    json_response_users_api([
                        'success' => true,
                        'roles' => [
                            ['value' => 'member', 'label' => '组员'],
                            ['value' => 'leader', 'label' => '组长'],
                            ['value' => 'supervisor', 'label' => '主管'],
                            ['value' => 'hr', 'label' => '人事'],
                            ['value' => 'manager', 'label' => '经理'],
                        ],
                    ]);
                    break;

                case 'add':
                    $new_username = trim($_POST['new_username'] ?? '');
                    $new_password = $_POST['new_password'] ?? '';
                    $real_name = trim($_POST['real_name'] ?? '');
                    $role = trim($_POST['role'] ?? 'member');

                    if (empty($new_username) || empty($new_password) || empty($real_name)) {
                        json_response_users_api(['success' => false, 'message' => '用户名、密码、姓名不能为空'], 400);
                    }
                    if (!in_array($role, ['member', 'leader', 'supervisor', 'hr', 'manager'], true)) {
                        json_response_users_api(['success' => false, 'message' => '无效的角色'], 400);
                    }

                    $check = $pdo->prepare("SELECT id FROM users WHERE username = ?");
                    $check->execute([$new_username]);
                    if ($check->fetch()) {
                        json_response_users_api(['success' => false, 'message' => '用户名 ' . $new_username . ' 已存在'], 400);
                    }

                    $hashed_password = password_hash($new_password, PASSWORD_DEFAULT);
                    $sector = trim($_POST['sector'] ?? '');
                    $skip_newbie = isset($_POST['skip_newbie']) ? 1 : 0;
                    $stmt = $pdo->prepare("INSERT INTO users (username, password, plain_password, real_name, role, company_id, status, sector, skip_newbie) VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?)");
                    $stmt->execute([$new_username, $hashed_password, $new_password, $real_name, $role, $api_company_id, $sector, $skip_newbie]);
                    auto_collect_reserved_sectors($pdo, $sector);
                    json_response_users_api(['success' => true, 'message' => '用户 ' . $real_name . ' 添加成功', 'user_id' => (int)$pdo->lastInsertId()]);
                    break;

                case 'update':
                    $user_id = (int)($_POST['user_id'] ?? 0);
                    $real_name = trim($_POST['real_name'] ?? '');
                    $role = trim($_POST['role'] ?? '');
                    $status = trim($_POST['status'] ?? '');
                    $new_password = trim($_POST['password'] ?? '');

                    if ($user_id <= 0) json_response_users_api(['success' => false, 'message' => '无效的用户ID'], 400);
                    if (!in_array($role, ['member', 'leader', 'supervisor', 'hr', 'manager'], true)) {
                        json_response_users_api(['success' => false, 'message' => '无效的角色'], 400);
                    }
                    if (!in_array($status, ['active', 'inactive'], true)) {
                        json_response_users_api(['success' => false, 'message' => '无效的状态'], 400);
                    }

                    $stmt = $pdo->prepare("SELECT * FROM users WHERE id = ? AND company_id = ?");
                    $stmt->execute([$user_id, $api_company_id]);
                    $target = $stmt->fetch(PDO::FETCH_ASSOC);
                    if (!$target) {
                        json_response_users_api(['success' => false, 'message' => '用户不存在'], 404);
                    }
                    if ((int)$target['is_super_admin'] && !$api_user['is_super_admin']) {
                        json_response_users_api(['success' => false, 'message' => '不能修改超级管理员'], 403);
                    }

                    $pdo->prepare("UPDATE users SET real_name = ?, role = ?, status = ?, sector = ?, skip_newbie = ? WHERE id = ? AND company_id = ?")
                        ->execute([$real_name ?: $target['real_name'], $role, $status, trim($_POST['sector'] ?? ''), isset($_POST['skip_newbie']) ? 1 : 0, $user_id, $api_company_id]);
                    auto_collect_reserved_sectors($pdo, $_POST['sector'] ?? '');

                    if (!empty($new_password)) {
                        $hashed = password_hash($new_password, PASSWORD_DEFAULT);
                        $pdo->prepare("UPDATE users SET password = ?, plain_password = ? WHERE id = ? AND company_id = ?")
                            ->execute([$hashed, $new_password, $user_id, $api_company_id]);
                    }

                    json_response_users_api(['success' => true, 'message' => '用户信息已更新']);
                    break;

                case 'toggle_status':
                    $user_id = (int)($_POST['user_id'] ?? 0);
                    $new_status = trim($_POST['status'] ?? '');

                    if ($user_id <= 0) json_response_users_api(['success' => false, 'message' => '无效的用户ID'], 400);
                    if (!in_array($new_status, ['active', 'inactive'], true)) {
                        json_response_users_api(['success' => false, 'message' => '无效的状态'], 400);
                    }
                    if ($user_id === (int)$api_user['id'] && !$api_user['is_super_admin']) {
                        json_response_users_api(['success' => false, 'message' => '不能修改自己的状态'], 403);
                    }

                    $stmt = $pdo->prepare("SELECT real_name, is_super_admin FROM users WHERE id = ? AND company_id = ?");
                    $stmt->execute([$user_id, $api_company_id]);
                    $target = $stmt->fetch(PDO::FETCH_ASSOC);
                    if (!$target) {
                        json_response_users_api(['success' => false, 'message' => '用户不存在'], 404);
                    }
                    if ((int)$target['is_super_admin'] && !$api_user['is_super_admin']) {
                        json_response_users_api(['success' => false, 'message' => '不能修改超级管理员'], 403);
                    }

                    if ($new_status === 'inactive') {
                        $pdo->prepare("UPDATE users SET status = ?, resignation_date = CURDATE() WHERE id = ? AND company_id = ?")->execute([$new_status, $user_id, $api_company_id]);
                    } else {
                        $pdo->prepare("UPDATE users SET status = ?, resignation_date = NULL WHERE id = ? AND company_id = ?")->execute([$new_status, $user_id, $api_company_id]);
                    }
                    json_response_users_api(['success' => true, 'message' => '用户状态已更新']);
                    break;

                case 'delete':
                    $user_id = (int)($_POST['user_id'] ?? 0);

                    if ($user_id <= 0) json_response_users_api(['success' => false, 'message' => '无效的用户ID'], 400);
                    if ($user_id === (int)$api_user['id'] && !$api_user['is_super_admin']) {
                        json_response_users_api(['success' => false, 'message' => '不能删除自己的账号'], 403);
                    }

                    $stmt = $pdo->prepare("SELECT real_name, is_super_admin FROM users WHERE id = ? AND company_id = ?");
                    $stmt->execute([$user_id, $api_company_id]);
                    $target = $stmt->fetch(PDO::FETCH_ASSOC);
                    if (!$target) {
                        json_response_users_api(['success' => false, 'message' => '用户不存在'], 404);
                    }
                    if ((int)$target['is_super_admin'] && !$api_user['is_super_admin']) {
                        json_response_users_api(['success' => false, 'message' => '不能删除超级管理员'], 403);
                    }

                    $pdo->prepare("DELETE FROM users WHERE id = ? AND company_id = ?")->execute([$user_id, $api_company_id]);
                    json_response_users_api(['success' => true, 'message' => '用户 ' . $target['real_name'] . ' 已删除']);
                    break;

                case 'permission_profiles':
                    ensure_permission_profiles_table($pdo);
                    $profiles = load_permission_profiles($pdo, $api_company_id);
                    $out = [];
                    foreach ($profiles as $pp) {
                        $out[] = [
                            'id' => (int)$pp['id'],
                            'company_id' => (int)$pp['company_id'],
                            'name' => $pp['name'] ?? '',
                            'description' => $pp['description'] ?? '',
                            'permissions' => profile_row_to_flat($pp),
                        ];
                    }
                    json_response_users_api(['success' => true, 'profiles' => $out, 'fields' => permission_profile_fields()]);
                    break;

                case 'permission_profile_save':
                    $profile_id = (int)($_POST['id'] ?? 0);
                    $profile_name = trim($_POST['name'] ?? '');
                    if ($profile_name === '') {
                        json_response_users_api(['success' => false, 'message' => '权限组名称不能为空'], 400);
                    }
                    ensure_permission_profiles_table($pdo);
                    $profile_values = [];
                    foreach (permission_profile_fields() as $col => $lbl) {
                        $profile_values[$col] = (int)($_POST['permissions'][$col] ?? 0) === 1 ? 1 : 0;
                    }
                    foreach ($profile_values as $col => $v) {
                        if ($v === 1 && !api_can_grant_permission($api_user, $col)) {
                            json_response_users_api(['success' => false, 'message' => '你尚未拥有「' . api_permission_label($col) . '」权限，不能把它放进权限组'], 403);
                        }
                    }
                    $ok = insert_or_update_permission_profile($pdo, $api_company_id, $profile_id, $profile_name, trim($_POST['description'] ?? ''), $profile_values);
                    if (!$ok) {
                        json_response_users_api(['success' => false, 'message' => '权限组保存失败'], 500);
                    }
                    json_response_users_api(['success' => true, 'message' => $profile_id ? '权限组已更新' : '权限组已创建']);
                    break;

                case 'permission_profile_delete':
                    $profile_id = (int)($_POST['id'] ?? 0);
                    if ($profile_id <= 0) {
                        json_response_users_api(['success' => false, 'message' => '无效的权限组ID'], 400);
                    }
                    ensure_permission_profiles_table($pdo);
                    $stmt = $pdo->prepare("SELECT company_id FROM permission_profiles WHERE id = ?");
                    $stmt->execute([$profile_id]);
                    $row = $stmt->fetch(PDO::FETCH_ASSOC);
                    if (!$row || (int)$row['company_id'] !== $api_company_id) {
                        json_response_users_api(['success' => false, 'message' => '权限组不存在或无权操作'], 404);
                    }
                    $pdo->prepare("DELETE FROM permission_profiles WHERE id = ? AND company_id = ?")
                        ->execute([$profile_id, $api_company_id]);
                    $pdo->prepare("UPDATE users SET permission_profile_id = NULL WHERE permission_profile_id = ? AND company_id = ?")
                        ->execute([$profile_id, $api_company_id]);
                    json_response_users_api(['success' => true, 'message' => '权限组已删除']);
                    break;

                case 'permission_profile_apply':
                    $user_id = (int)($_POST['user_id'] ?? 0);
                    $profile_id = (int)($_POST['profile_id'] ?? 0);
                    if ($user_id <= 0) {
                        json_response_users_api(['success' => false, 'message' => '无效的用户ID'], 400);
                    }
                    ensure_permission_profiles_table($pdo);
                    $stmt = $pdo->prepare("SELECT id, is_super_admin FROM users WHERE id = ? AND company_id = ?");
                    $stmt->execute([$user_id, $api_company_id]);
                    $target = $stmt->fetch(PDO::FETCH_ASSOC);
                    if (!$target) {
                        json_response_users_api(['success' => false, 'message' => '用户不存在'], 404);
                    }
                    if ((int)$target['is_super_admin'] && !$api_user['is_super_admin']) {
                        json_response_users_api(['success' => false, 'message' => '不能修改超级管理员权限'], 403);
                    }
                    if ($profile_id <= 0) {
                        $pdo->prepare("UPDATE users SET permission_profile_id = NULL WHERE id = ? AND company_id = ?")
                            ->execute([$user_id, $api_company_id]);
                        json_response_users_api(['success' => true, 'message' => '已解除权限组']);
                        break;
                    }
                    $stmt = $pdo->prepare("SELECT * FROM permission_profiles WHERE id = ? AND company_id = ?");
                    $stmt->execute([$profile_id, $api_company_id]);
                    $profile = $stmt->fetch(PDO::FETCH_ASSOC);
                    if (!$profile) {
                        json_response_users_api(['success' => false, 'message' => '权限组不存在'], 404);
                    }
                    if (!$api_user['is_super_admin'] && $user_id === (int)$api_user['id']) {
                        json_response_users_api(['success' => false, 'message' => '不能把权限组套用到自己账号'], 403);
                    }
                    $flat = profile_row_to_flat($profile);
                    foreach ($flat as $col => $v) {
                        if ($v === 1 && !api_can_grant_permission($api_user, $col)) {
                            json_response_users_api(['success' => false, 'message' => '你尚未拥有「' . api_permission_label($col) . '」权限，不能套用该权限组'], 403);
                        }
                    }
                    ensure_user_permission_columns($pdo);
                    $ok = apply_permission_profile_to_user($pdo, $user_id, $api_company_id, $flat, $profile_id);
                    if (!$ok) {
                        json_response_users_api(['success' => false, 'message' => '权限组套用失败'], 500);
                    }
                    json_response_users_api(['success' => true, 'message' => '权限组已套用']);
                    break;

                default:
                    json_response_users_api(['success' => false, 'message' => '未知操作: ' . $ACTION], 400);
            }
        } catch (PDOException $e) {
            json_response_users_api(['success' => false, 'message' => '数据库错误: ' . $e->getMessage()], 500);
        } catch (Exception $e) {
            json_response_users_api(['success' => false, 'message' => '服务器错误: ' . $e->getMessage()], 500);
        }
        exit; // API 处理完毕，不执行下面的 Web 页面
    }
}
// ============================================================
// 以下是 Web 页面模式（session 认证，不走 API）
// ============================================================

check_auth();
 

$user = get_logged_user($pdo);
$company_id = get_current_company_id($pdo);

// 判断是否为超级管理员
$is_super_admin = is_super_admin($pdo);

// 页面权限门禁：需开启「用户管理」权限方可访问（超级管理员始终放行）
if (!$is_super_admin && (int)($user['allow_user_management'] ?? 0) !== 1) {
    http_response_code(403);
    echo '<!DOCTYPE html><html lang="zh-CN"><head><meta charset="UTF-8"><title>无权访问</title></head>'
       . '<body style="font-family:system-ui,sans-serif;background:#f5f7fa;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;">'
       . '<div style="background:#fff;padding:40px 48px;border-radius:12px;box-shadow:0 10px 30px rgba(0,0,0,.08);text-align:center;max-width:420px;">'
       . '<div style="font-size:44px;">🔒</div>'
       . '<h2 style="margin:16px 0 8px;color:#d93026;">无权访问用户管理</h2>'
       . '<p style="color:#666;line-height:1.6;margin:0 0 20px;">你的账号未开启「用户管理」权限，请联系上级在权限组中为你开启后再访问。</p>'
       . '<a href="dashboard.php" style="display:inline-block;padding:10px 24px;background:#1976d2;color:#fff;border-radius:6px;text-decoration:none;">返回首页</a>'
       . '</div></body></html>';
    exit;
}

function user_column_exists($pdo, $column) {
    $safeColumn = preg_replace('/[^a-z0-9_]/i', '', $column);
    try {
        $stmt = $pdo->query("SHOW COLUMNS FROM users LIKE '" . $safeColumn . "'");
        return $stmt && $stmt->rowCount() > 0;
    } catch (Exception $e) {
        $stmt = $pdo->prepare("SELECT 1 FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'users' AND COLUMN_NAME = ?");
        $stmt->execute([$safeColumn]);
        return (bool)$stmt->fetchColumn();
    }
}

function phone_exists_except($pdo, $phone, $except_user_id = 0) {
    if ($phone === '') return false;
    $stmt = $pdo->prepare("SELECT id FROM users WHERE phone = ? AND id != ?");
    $stmt->execute([$phone, (int)$except_user_id]);
    return (bool)$stmt->fetch();
}

function ensure_user_permission_columns($pdo) {
    global $module_permissions;
    $columns = [
        'allow_view_revenue' => "ALTER TABLE users ADD COLUMN allow_view_revenue TINYINT(1) NOT NULL DEFAULT 0 COMMENT '允许查看收益'",
        'allow_view_password' => "ALTER TABLE users ADD COLUMN allow_view_password TINYINT(1) NOT NULL DEFAULT 0 COMMENT '允许查看账号密码'",
        'allow_manage_users' => "ALTER TABLE users ADD COLUMN allow_manage_users TINYINT(1) NOT NULL DEFAULT 0 COMMENT '允许添加用户和修改权限'",
        'managed_group_ids' => "ALTER TABLE users ADD COLUMN managed_group_ids VARCHAR(255) DEFAULT NULL COMMENT '可管理小组ID，逗号分隔'",
        'login_prompt_enabled' => "ALTER TABLE users ADD COLUMN login_prompt_enabled TINYINT(1) NOT NULL DEFAULT 0 COMMENT '登录提示开关'",
        'login_prompt_text' => "ALTER TABLE users ADD COLUMN login_prompt_text TEXT DEFAULT NULL COMMENT '登录提示内容'",
        'checkin_reminder_enabled' => "ALTER TABLE users ADD COLUMN checkin_reminder_enabled TINYINT(1) NOT NULL DEFAULT 0 COMMENT '签到提醒开关'",
        'sector' => "ALTER TABLE users ADD COLUMN sector VARCHAR(255) DEFAULT NULL COMMENT '视频解说赛道，逗号分隔多选，如：动漫,体育'",
        'skip_newbie' => "ALTER TABLE users ADD COLUMN skip_newbie TINYINT(1) NOT NULL DEFAULT 0 COMMENT '手动跳过新手期(1=已跳过，不显示新手标签)'",
        'permission_profile_id' => "ALTER TABLE users ADD COLUMN permission_profile_id INT DEFAULT NULL COMMENT '关联的权限组ID，权限组变更时自动同步权限'"
    ];
    foreach ($module_permissions as $column => $label) {
        $columns[$column] = "ALTER TABLE users ADD COLUMN {$column} TINYINT(1) NOT NULL DEFAULT 0 COMMENT '{$label}权限'";
    }
    foreach ($columns as $column => $sql) {
        if (!user_column_exists($pdo, $column)) {
            $pdo->exec($sql);
        }
    }
}

/**
 * 权限组：打包一组 allow_* 权限，给员工选组一键套用（复制到用户自身字段）
 */
function ensure_permission_profiles_table($pdo) {
    $stmt = $pdo->query("SHOW TABLES LIKE 'permission_profiles'");
    if ($stmt && $stmt->rowCount() > 0) {
        // 表已存在，检查并补全缺失的字段
        $profile_fields = permission_profile_fields();
        foreach ($profile_fields as $col => $label) {
            $check = $pdo->query("SHOW COLUMNS FROM permission_profiles LIKE '{$col}'");
            if (!$check || !$check->fetch()) {
                $pdo->exec("ALTER TABLE permission_profiles ADD COLUMN {$col} TINYINT(1) NOT NULL DEFAULT 0 COMMENT '{$label}'");
            }
        }
        // 额外检查功能开关字段
        foreach (['login_prompt_enabled' => '登录提示开关', 'checkin_reminder_enabled' => '签到提醒开关'] as $col => $label) {
            $check = $pdo->query("SHOW COLUMNS FROM permission_profiles LIKE '{$col}'");
            if (!$check || !$check->fetch()) {
                $pdo->exec("ALTER TABLE permission_profiles ADD COLUMN {$col} TINYINT(1) NOT NULL DEFAULT 0 COMMENT '{$label}'");
            }
        }
        return;
    }
    $ddl = "CREATE TABLE IF NOT EXISTS permission_profiles (
        id INT AUTO_INCREMENT PRIMARY KEY,
        company_id INT NOT NULL DEFAULT 0,
        name VARCHAR(100) NOT NULL,
        description VARCHAR(255) DEFAULT NULL,
        allow_video_download TINYINT(1) DEFAULT 0,
        allow_data_dashboard TINYINT(1) DEFAULT 0,
        allow_data_submit TINYINT(1) DEFAULT 0,
        allow_baowenku TINYINT(1) DEFAULT 0,
        allow_my_payslip TINYINT(1) DEFAULT 0,
        allow_user_management TINYINT(1) DEFAULT 0,
        allow_data_report TINYINT(1) DEFAULT 0,
        allow_account_detail TINYINT(1) DEFAULT 0,
        allow_account_revenue TINYINT(1) DEFAULT 0,
        allow_salary_management TINYINT(1) DEFAULT 0,
        allow_finance TINYINT(1) DEFAULT 0,
        allow_operation_log TINYINT(1) DEFAULT 0,
        allow_wish_wall TINYINT(1) DEFAULT 0,
        allow_memo TINYINT(1) DEFAULT 0,
        allow_sms_management TINYINT(1) DEFAULT 0,
        allow_2fa TINYINT(1) DEFAULT 0,
        allow_profile TINYINT(1) DEFAULT 0,
        allow_company_settings TINYINT(1) DEFAULT 0,
        allow_remote_desktop TINYINT(1) DEFAULT 0,
        allow_remote_management TINYINT(1) DEFAULT 0,
        allow_view_revenue TINYINT(1) DEFAULT 0,
        allow_view_password TINYINT(1) DEFAULT 0,
        allow_manage_users TINYINT(1) DEFAULT 0,
        login_prompt_enabled TINYINT(1) DEFAULT 0,
        checkin_reminder_enabled TINYINT(1) DEFAULT 0,
        created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
        updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
        INDEX idx_company (company_id)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4";
    $pdo->exec($ddl);
}

/**
 * 权限组可选字段（含模块权限 + 额外权限）
 */
function permission_profile_fields() {
    global $module_permissions;
    $fields = [];
    foreach ($module_permissions as $col => $lbl) {
        $fields[$col] = $lbl;
    }
    $fields['allow_view_revenue'] = '允许查看收益';
    $fields['allow_view_password'] = '允许查看密码';
    $fields['allow_manage_users'] = '允许添加用户和修改权限';
    $fields['allow_2fa'] = '安全链接(2FA)';
    $fields['login_prompt_enabled'] = '登录提示';
    $fields['checkin_reminder_enabled'] = '签到提醒';
    return $fields;
}

function load_permission_profiles($pdo, $company_id) {
    $stmt = $pdo->prepare("SELECT * FROM permission_profiles WHERE company_id = ? ORDER BY id DESC");
    $stmt->execute([(int)$company_id]);
    return $stmt->fetchAll();
}

function load_permission_profiles_by_company($pdo, $company_id) {
    $stmt = $pdo->prepare("SELECT * FROM permission_profiles WHERE company_id = ? ORDER BY id DESC");
    $stmt->execute([(int)$company_id]);
    return $stmt->fetchAll();
}

/**
 * 权限组为空时，自动创建本公司默认权限组（company_id = 当前公司）：
 *  1.组员 2.人事（组员+用户管理+工资管理+账号详情+公司设置） 3.经理（除远程桌面外全部权限）
 */
function ensure_default_permission_profiles($pdo, $company_id) {
    $stmt = $pdo->prepare("SELECT id FROM permission_profiles WHERE company_id = ? LIMIT 1");
    $stmt->execute([(int)$company_id]);
    if ($stmt->fetch()) {
        return false;
    }
    $fields = permission_profile_fields();

    $zero = [];
    foreach ($fields as $col => $lbl) { $zero[$col] = 0; }

    // 1. 组员
    $member = $zero;
    foreach (['allow_video_download','allow_data_dashboard','allow_data_submit','allow_baowenku','allow_my_payslip','allow_profile'] as $c) {
        if (isset($member[$c])) $member[$c] = 1;
    }
    // 2. 人事 = 组员 + 用户管理 + 工资管理 + 账号详情 + 公司设置
    $hr = $member;
    foreach (['allow_user_management','allow_salary_management','allow_account_detail','allow_company_settings'] as $c) {
        if (isset($hr[$c])) $hr[$c] = 1;
    }
    // 3. 经理 = 除远程桌面（及登录提示/签到提醒功能开关）外的全部权限
    $manager = $zero;
    foreach ($fields as $col => $lbl) {
        if (in_array($col, ['allow_remote_desktop','login_prompt_enabled','checkin_reminder_enabled'], true)) continue;
        $manager[$col] = 1;
    }

    $id = insert_or_update_permission_profile($pdo, (int)$company_id, 0, '组员', '默认组员权限', $member);
    insert_or_update_permission_profile($pdo, (int)$company_id, 0, '人事', '默认人事权限', $hr);
    insert_or_update_permission_profile($pdo, (int)$company_id, 0, '经理', '默认经理权限（除远程桌面）', $manager);
    return $id ? true : false;
}

function profile_row_to_flat($row) {
    $flat = [];
    foreach (permission_profile_fields() as $col => $lbl) {
        $flat[$col] = (int)($row[$col] ?? 0) === 1 ? 1 : 0;
    }
    return $flat;
}

function insert_or_update_permission_profile($pdo, $company_id, $id, $name, $description, $values) {
    $fields = array_keys($values);
    if ($id) {
        $sets = [];
        $params = [];
        foreach ($fields as $col) {
            $sets[] = "{$col} = ?";
            $params[] = (int)$values[$col];
        }
        $sets[] = "name = ?";
        $sets[] = "description = ?";
        $sets[] = "updated_at = NOW()";
        $params[] = $name;
        $params[] = $description;
        $params[] = $id;
        $params[] = (int)$company_id;
        $stmt = $pdo->prepare("UPDATE permission_profiles SET " . implode(', ', $sets) . " WHERE id = ? AND company_id = ?");
        return $stmt->execute($params);
    } else {
        $cols = array_merge(['company_id', 'name', 'description'], $fields);
        $marks = array_fill(0, count($cols), '?');
        $params = [(int)$company_id, $name, $description];
        foreach ($fields as $col) {
            $params[] = (int)$values[$col];
        }
        $stmt = $pdo->prepare("INSERT INTO permission_profiles (" . implode(', ', $cols) . ") VALUES (" . implode(', ', $marks) . ")");
        return $stmt->execute($params);
    }
}

function apply_permission_profile_to_user($pdo, $user_id, $company_id, $flat, $profile_id = 0) {
    $sets = [];
    $params = [];
    foreach ($flat as $col => $val) {
        $sets[] = "{$col} = ?";
        $params[] = (int)$val;
    }
    $sets[] = 'permission_profile_id = ?';
    $params[] = $profile_id > 0 ? (int)$profile_id : null;
    $params[] = $user_id;
    $params[] = $company_id;
    $stmt = $pdo->prepare("UPDATE users SET " . implode(', ', $sets) . " WHERE id = ? AND company_id = ?");
    return $stmt->execute($params);
}

function bool_post($name) {
    return isset($_POST[$name]) ? 1 : 0;
}

function collect_module_permissions_from_post() {
    global $module_permissions;
    $values = [];
    foreach ($module_permissions as $column => $label) {
        $values[$column] = bool_post($column);
    }
    return $values;
}

function update_user_module_permissions($pdo, $user_id, $company_id, $values) {
    if (empty($values)) return;
    $sets = [];
    $params = [];
    foreach ($values as $column => $value) {
        $sets[] = "{$column} = ?";
        $params[] = (int)$value;
    }
    $params[] = $user_id;
    $params[] = $company_id;
    $stmt = $pdo->prepare("UPDATE users SET " . implode(', ', $sets) . " WHERE id = ? AND company_id = ?");
    $stmt->execute($params);
}

function module_permissions_for_row($row) {
    global $module_permissions;
    $result = [];
    foreach ($module_permissions as $column => $label) {
        $result[$column] = (int)($row[$column] ?? 0);
    }
    return $result;
}

function effective_module_permissions_for_row($row) {
    global $module_permissions;
    $result = [];
    foreach ($module_permissions as $column => $label) {
        // 所有模块权限均从数据库读取，不因管理员角色自动开启
        $result[$column] = (int)($row[$column] ?? 0) === 1 ? 1 : 0;
    }
    return $result;
}

function can_user_view_revenue_by_row($row) {
    return (int)($row['is_super_admin'] ?? 0) === 1
        || in_array(($row['role'] ?? ''), ['manager', 'admin', 'super_admin'], true)
        || (int)($row['allow_view_revenue'] ?? 0) === 1;
}

function can_user_view_password_by_row($row) {
    return (int)($row['is_super_admin'] ?? 0) === 1
        || in_array(($row['role'] ?? ''), ['manager', 'admin', 'super_admin'], true)
        || (int)($row['allow_view_password'] ?? 0) === 1;
}

function can_user_manage_users_by_row($row) {
    return (int)($row['is_super_admin'] ?? 0) === 1
        || in_array(($row['role'] ?? ''), ['manager', 'admin', 'super_admin'], true)
        || (int)($row['allow_manage_users'] ?? 0) === 1;
}

function parse_group_ids($value) {
    $parts = preg_split('/[,\s，、]+/', trim((string)$value));
    $ids = [];
    foreach ($parts as $part) {
        if ($part !== '' && is_numeric($part)) $ids[] = (int)$part;
    }
    return array_values(array_unique($ids));
}

function can_manage_group_id($is_super_admin, $user, $group_id) {
    if ($is_super_admin || ($user['role'] ?? '') === 'manager') return true;
    if ((int)($user['allow_manage_users'] ?? 0) !== 1) return false;
    $allowed = parse_group_ids($user['managed_group_ids'] ?? '');
    if (empty($allowed)) return (int)$group_id === (int)($user['group_id'] ?? 0);
    return in_array((int)$group_id, $allowed, true);
}

function current_user_can_edit_target($is_super_admin, $user, $target) {
    if ($is_super_admin || ($user['role'] ?? '') === 'manager') return true;
    if ((int)($user['allow_manage_users'] ?? 0) !== 1) return false;
    return can_manage_group_id($is_super_admin, $user, $target['group_id'] ?? 0);
}

function render_user_permission_badges($row) {
    global $module_permissions, $user, $is_super_admin, $selected_company_id;

    $target_id = intval($row['id']);
    $target_is_super = intval($row['is_super_admin'] ?? 0) === 1;
    $is_self = $target_id === intval($user['id']);

    // 判断当前用户是否可以修改目标用户的权限
    $can_edit = true;
    if ($target_is_super && !$is_super_admin) {
        $can_edit = false;
    }
    if ($is_self && !$is_super_admin) {
        $can_edit = false;
    }
    if (!current_user_can_edit_target($is_super_admin, $user, $row)) {
        $can_edit = false;
    }

    $company_param = $is_super_admin ? '&company_id=' . $selected_company_id : '';

    // 辅助函数：输出一个可点击或不可点击的权限 badge
    $render_badge = function($perm_key, $label, $on) use ($can_edit, $target_id, $selected_company_id, $is_super_admin) {
        $new_val = $on ? 0 : 1;

        if ($can_edit) {
            $company_val = $is_super_admin ? (int)$selected_company_id : 0;
            echo '<span class="perm-badge perm-clickable ' . ($on ? 'perm-on' : 'perm-off') . '" onclick="togglePermBadge(this, ' . $target_id . ', \'' . $perm_key . '\', ' . $new_val . ', \'' . htmlspecialchars($label, ENT_QUOTES) . '\', ' . $company_val . ');">' . htmlspecialchars($label) . ($on ? '开' : '关') . '</span>';
        } else {
            echo '<span class="perm-badge ' . ($on ? 'perm-on' : 'perm-off') . '">' . htmlspecialchars($label) . ($on ? '开' : '关') . '</span>';
        }
    };

    $revenue = can_user_view_revenue_by_row($row);
    $password = can_user_view_password_by_row($row);
    $render_badge('allow_view_revenue', '收益', $revenue);
    $render_badge('allow_view_password', '密码', $password);
    $render_badge('allow_manage_users', '管理', can_user_manage_users_by_row($row));

    // 所有用户（含管理员）均显示各模块实际权限
    $badge_html = '';
    ob_start();
    $module_perms = effective_module_permissions_for_row($row);
    foreach ($module_permissions as $column => $label) {
        $on = (int)($module_perms[$column] ?? 0) === 1;
        $render_badge($column, $label, $on);
    }
    if (!empty($row['managed_group_ids'])) {
        echo '<span class="perm-badge perm-on">管' . htmlspecialchars($row['managed_group_ids']) . '组</span>';
    }
    $has_prompt = (int)($row['login_prompt_enabled'] ?? 0) === 1;
    $render_badge('login_prompt_enabled', '登录提示', $has_prompt);
    $has_checkin = (int)($row['checkin_reminder_enabled'] ?? 0) === 1;
    $render_badge('checkin_reminder_enabled', '签到提醒', $has_checkin);
    $badge_html = ob_get_clean();

    echo '<div class="perm-fold" id="permfold-' . $target_id . '">';
    echo '<div class="perm-badges">' . $badge_html . '</div>';
    echo '<div class="perm-fold-toggle" onclick="togglePermFold(' . $target_id . ')">展开 <span class="pf-caret">▾</span></div>';
    echo '</div>';
}

function has_any_permission($row) {
    global $module_permissions;
    foreach ($module_permissions as $col => $lbl) {
        if ((int)($row[$col] ?? 0) === 1) return true;
    }
    if ((int)($row['allow_view_revenue'] ?? 0) === 1) return true;
    if ((int)($row['allow_view_password'] ?? 0) === 1) return true;
    if ((int)($row['allow_manage_users'] ?? 0) === 1) return true;
    if ((int)($row['login_prompt_enabled'] ?? 0) === 1) return true;
    if ((int)($row['checkin_reminder_enabled'] ?? 0) === 1) return true;
    return false;
}

/**
 * 用户权限组选择列：根据用户当前权限自动匹配一个最接近的权限组，可选组一键套用
 */
function render_profile_select($u, $permission_profiles, $user, $is_super_admin, $selected_company_id) {
    if (!function_exists('can_user_manage_users_by_row') || !can_user_manage_users_by_row($user)) {
        echo '<span class="pp-no-perm">—</span>';
        return;
    }
    if (empty($permission_profiles)) {
        echo '<span class="pp-no-perm">暂无权限组</span>';
        return;
    }
    // 优先按已关联的权限组高亮；未关联时回退到按当前权限完全匹配
    $matched = (int)($u['permission_profile_id'] ?? 0);
    if ($matched <= 0) {
        $flat = profile_row_to_flat(array_merge(
            array_fill_keys(array_keys(permission_profile_fields()), 0),
            array_intersect_key($u, permission_profile_fields())
        ));
        foreach ($permission_profiles as $pp) {
            if (profile_row_to_flat($pp) === $flat) { $matched = (int)$pp['id']; break; }
        }
    }
    echo '<select class="pp-sel" onchange="applyUserProfile(this, ' . (int)$u['id'] . ', this.value)">';
    echo '<option value="">未设权限组</option>';
    foreach ($permission_profiles as $pp) {
        $sel = ((int)$pp['id'] === $matched) ? ' selected' : '';
        echo '<option value="' . (int)$pp['id'] . '"' . $sel . '>' . htmlspecialchars($pp['name']) . '</option>';
    }
    echo '</select>';
}

try {
    ensure_user_permission_columns($pdo);
} catch (Exception $e) {
    error_log('用户权限字段初始化失败: ' . $e->getMessage());
}

try {
    ensure_permission_profiles_table($pdo);
} catch (Exception $e) {
    error_log('权限组表初始化失败: ' . $e->getMessage());
}

$stmt = $pdo->prepare("SELECT * FROM users WHERE id = ?");
$stmt->execute([$user['id']]);
$fresh_user = $stmt->fetch();
if ($fresh_user) $user = $fresh_user;

// AJAX: 切换权限（POST，走 session 认证，返回 JSON）
if ($_SERVER['REQUEST_METHOD'] === 'POST' && ($_POST['action'] ?? '') === 'toggle_permission_ajax') {
    header('Content-Type: application/json; charset=utf-8');

    $toggle_id = intval($_POST['user_id'] ?? 0);
    $perm_column = trim($_POST['permission'] ?? '');
    $perm_value = intval($_POST['value'] ?? -1);
    $target_company_id = $company_id;
    if ($is_super_admin && !empty($_POST['company_id'])) {
        $target_company_id = intval($_POST['company_id']);
    }

    $valid_permissions = array_merge(
        array_keys($module_permissions),
        ['allow_view_revenue', 'allow_view_password', 'allow_manage_users', 'login_prompt_enabled', 'checkin_reminder_enabled']
    );

    if (!can_user_manage_users_by_row($user)) {
        echo json_encode(['success' => false, 'message' => '无用户管理权限']);
        exit;
    }
    if (!in_array($perm_column, $valid_permissions, true)) {
        echo json_encode(['success' => false, 'message' => '无效的权限字段']);
        exit;
    }
    if ($perm_value !== 0 && $perm_value !== 1) {
        echo json_encode(['success' => false, 'message' => '无效的权限值']);
        exit;
    }
    if ($toggle_id == $user['id'] && !$is_super_admin) {
        echo json_encode(['success' => false, 'message' => '不能修改自己的权限']);
        exit;
    }

    try {
        $stmt = $pdo->prepare("SELECT real_name, is_super_admin, group_id, {$perm_column} FROM users WHERE id = ? AND company_id = ?");
        $stmt->execute([$toggle_id, $target_company_id]);
        $target = $stmt->fetch();
    } catch (Exception $e) {
        echo json_encode(['success' => false, 'message' => '权限字段不存在']);
        exit;
    }

    if (!$target) {
        echo json_encode(['success' => false, 'message' => '用户不存在']);
        exit;
    }
    if (!current_user_can_edit_target($is_super_admin, $user, $target)) {
        echo json_encode(['success' => false, 'message' => '只能操作你可管理小组内的用户']);
        exit;
    }
    if (intval($target['is_super_admin']) && !$is_super_admin) {
        echo json_encode(['success' => false, 'message' => '不能修改超级管理员权限']);
        exit;
    }

    $old_val = intval($target[$perm_column] ?? 0);
    $stmt = $pdo->prepare("UPDATE users SET {$perm_column} = ? WHERE id = ? AND company_id = ?");
    $stmt->execute([$perm_value, $toggle_id, $target_company_id]);

    $perm_labels = array_merge($module_permissions, [
        'allow_view_revenue' => '查看收益',
        'allow_view_password' => '查看密码',
        'allow_manage_users' => '管理用户',
        'login_prompt_enabled' => '登录提示',
        'checkin_reminder_enabled' => '签到提醒',
    ]);
    $perm_label = $perm_labels[$perm_column] ?? $perm_column;
    $action_text = $perm_value == 1 ? '开启' : '关闭';

    log_operation($pdo, $user['id'], $user['username'], $user['real_name'], 'toggle_permission', 'user', $toggle_id, $target['real_name'], json_encode([$perm_column => $old_val]), json_encode([$perm_column => $perm_value]), $target_company_id);

    echo json_encode(['success' => true, 'message' => "{$target['real_name']} 的「{$perm_label}」已{$action_text}"]);
    exit;
}

// 用户名查重（全局唯一；编辑时排除自身）
if (isset($_GET['check_username'])) {
    header('Content-Type: application/json; charset=utf-8');
    $cu = trim($_GET['check_username']);
    $cu_id = isset($_GET['user_id']) ? intval($_GET['user_id']) : 0;
    if ($cu === '') {
        echo json_encode(['success' => true, 'exists' => false]);
        exit;
    }
    $qs = $pdo->prepare("SELECT id FROM users WHERE username = ?");
    $qs->execute([$cu]);
    $crow = $qs->fetch();
    $exists = $crow && (int)$crow['id'] !== $cu_id ? true : false;
    echo json_encode(['success' => true, 'exists' => $exists]);
    exit;
}

// 客户端读取当前用户权限；员工只能读取自己，具备用户管理权限者可读取同公司员工
if (isset($_GET['action']) && $_GET['action'] === 'get_user_permissions') {
    header('Content-Type: application/json; charset=utf-8');
    $target_user_id = isset($_GET['user_id']) && is_numeric($_GET['user_id']) ? intval($_GET['user_id']) : intval($user['id']);
    $target_company_id = isset($_GET['company_id']) && is_numeric($_GET['company_id']) ? intval($_GET['company_id']) : intval($company_id);
    $can_query_other = can_user_manage_users_by_row($user);
    if (!$can_query_other && $target_user_id !== intval($user['id'])) {
        echo json_encode(['success' => false, 'message' => '权限不足']);
        exit;
    }
    try {
        $stmt = $pdo->prepare("SELECT * FROM users WHERE id = ? AND company_id = ?");
        $stmt->execute([$target_user_id, $target_company_id]);
        $target = $stmt->fetch();
        if (!$target) {
            echo json_encode(['success' => false, 'message' => '用户不存在']);
            exit;
        }
        $allow_revenue = can_user_view_revenue_by_row($target) ? 1 : 0;
        $allow_password = can_user_view_password_by_row($target) ? 1 : 0;
        $allow_manage = can_user_manage_users_by_row($target) ? 1 : 0;
        $module_perms = effective_module_permissions_for_row($target);
        $raw_module_perms = module_permissions_for_row($target);
        echo json_encode([
            'success' => true,
            'permissions' => [
                'allow_view_revenue' => $allow_revenue,
                'allow_employee_view_revenue' => $allow_revenue,
                'allow_view_password' => $allow_password,
                'allow_employee_view_password' => $allow_password,
                'allow_manage_users' => $allow_manage,
                'managed_group_ids' => $target['managed_group_ids'] ?? ''
            ] + $module_perms,
            'raw_permissions' => [
                'allow_view_revenue' => (int)($target['allow_view_revenue'] ?? 0),
                'allow_view_password' => (int)($target['allow_view_password'] ?? 0),
                'allow_manage_users' => (int)($target['allow_manage_users'] ?? 0),
                'managed_group_ids' => $target['managed_group_ids'] ?? ''
            ] + $raw_module_perms,
            'permission_labels' => $module_permissions,
            'allow_view_revenue' => $allow_revenue,
            'allow_employee_view_revenue' => $allow_revenue,
            'allow_view_password' => $allow_password,
            'allow_employee_view_password' => $allow_password,
            'allow_manage_users' => $allow_manage,
            'managed_group_ids' => $target['managed_group_ids'] ?? '',
            'permission_profile_id' => (int)($target['permission_profile_id'] ?? 0)
        ] + $module_perms);
    } catch (Exception $e) {
        echo json_encode(['success' => false, 'message' => $e->getMessage()]);
    }
    exit;
}

// 获取所有公司列表（供超级管理员选择）
$companies = [];
if ($is_super_admin) {
    $stmt = $pdo->query("SELECT id, company_name, status FROM companies ORDER BY company_name");
    $companies = $stmt->fetchAll();
}

// 超级管理员可以选择公司
$selected_company_id = $company_id;
if ($is_super_admin && isset($_GET['company_id']) && is_numeric($_GET['company_id'])) {
    $selected_company_id = intval($_GET['company_id']);
} elseif ($is_super_admin && isset($_POST['company_id']) && is_numeric($_POST['company_id'])) {
    $selected_company_id = intval($_POST['company_id']);
}

// 获取状态筛选参数
$status_filter = isset($_GET['status']) ? $_GET['status'] : '';

$message = '';
$error = '';
$is_scoped_user_manager = !$is_super_admin && ($user['role'] ?? '') !== 'manager' && (int)($user['allow_manage_users'] ?? 0) === 1;
$scope_group_ids = $is_scoped_user_manager ? parse_group_ids($user['managed_group_ids'] ?? '') : [];
if ($is_scoped_user_manager && empty($scope_group_ids) && !empty($user['group_id'])) {
    $scope_group_ids = [(int)$user['group_id']];
}

// ============================================================
// 新增用户
// ============================================================
if ($_SERVER['REQUEST_METHOD'] == 'POST' && isset($_POST['action']) && $_POST['action'] == 'add') {
    $username = trim($_POST['username']);
    $real_name = trim($_POST['real_name']);
    $phone = trim($_POST['phone'] ?? '');
    $phone = $phone !== '' ? $phone : null;
    $role = $_POST['role'];
    $plain_password = $_POST['password'];
    $hashed_password = password_hash($plain_password, PASSWORD_DEFAULT);
    $group_id = $_POST['group_id'] ?: null;
    $status = $_POST['status'] ?? 'active';
    $managed_group_ids = trim($_POST['managed_group_ids'] ?? '');
    $login_prompt_enabled = bool_post('login_prompt_enabled');
    $login_prompt_text = trim($_POST['login_prompt_text'] ?? '');
    $checkin_reminder_enabled = bool_post('checkin_reminder_enabled');
    $sector = trim($_POST['sector'] ?? '');
    $skip_newbie = isset($_POST['skip_newbie']) ? 1 : 0;

    // 检查用户名是否已存在（全局唯一，不加company_id）
    $check = $pdo->prepare("SELECT id FROM users WHERE username = ?");
    $check->execute([$username]);
    $existing = $check->fetch();

    // 检查联系电话是否已存在（全局唯一）
    $phone_dup = false;
    if (!empty($phone)) {
        $pcheck = $pdo->prepare("SELECT id FROM users WHERE phone = ?");
        $pcheck->execute([$phone]);
        $phone_dup = $pcheck->fetch();
    }

    if (!can_user_manage_users_by_row($user)) {
        $error = "没有添加用户权限！";
    } elseif (!can_manage_group_id($is_super_admin, $user, $group_id ?: 0)) {
        $error = "只能添加到你可管理的小组！";
    } elseif ($existing) {
        $error = "用户名 " . $username . " 已存在，请更换用户名！";
    } elseif ($phone_dup) {
        $error = "联系电话 " . $phone . " 已存在，请更换！";
    } else {
        // 确保 plain_password 字段存在
        if (!user_column_exists($pdo, 'plain_password')) {
            $pdo->exec("ALTER TABLE users ADD COLUMN plain_password VARCHAR(255) DEFAULT NULL");
        }

        $stmt = $pdo->prepare("INSERT INTO users (username, password, plain_password, real_name, phone, role, group_id, company_id, status, sector, managed_group_ids, login_prompt_enabled, login_prompt_text, checkin_reminder_enabled, skip_newbie) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)");
        $stmt->execute([$username, $hashed_password, $plain_password, $real_name, $phone, $role, $group_id, $selected_company_id, $status, $sector, $managed_group_ids, $login_prompt_enabled, $login_prompt_text, $checkin_reminder_enabled, $skip_newbie]);
        auto_collect_reserved_sectors($pdo, $sector);
        $new_user_id = $pdo->lastInsertId();

        // 选择了权限组则直接套用（权限以权限组为准）；非超管仅能套用自己已持有的权限
        $profile_choice = (int)($_POST['permission_profile'] ?? 0);
        if ($profile_choice > 0) {
            $pp = $pdo->prepare("SELECT * FROM permission_profiles WHERE id = ? AND company_id = ?");
            $pp->execute([$profile_choice, $selected_company_id]);
            $profile_row = $pp->fetch();
            if ($profile_row) {
                $flat_profile = profile_row_to_flat($profile_row);
                if (api_user_owns_profile_perms($user, $flat_profile)) {
                    apply_permission_profile_to_user($pdo, $new_user_id, $selected_company_id, $flat_profile, $profile_row['id']);
                }
            }
        }

        log_operation($pdo, $user['id'], $user['username'], $user['real_name'], 'add', 'user', $new_user_id, $real_name, null, json_encode(['username' => $username, 'role' => $role, 'group_id' => $group_id, 'status' => $status, 'managed_group_ids' => $managed_group_ids, 'login_prompt_enabled' => $login_prompt_enabled, 'login_prompt_text' => $login_prompt_text, 'checkin_reminder_enabled' => $checkin_reminder_enabled]), $selected_company_id);
        $message = "用户 " . $real_name . " 添加成功！密码：" . $plain_password;
    }
}
// ============================================================
// 编辑用户（同时更新明文密码）
// ============================================================
if ($_SERVER['REQUEST_METHOD'] == 'POST' && isset($_POST['action']) && $_POST['action'] == 'edit') {
    $edit_id = $_POST['user_id'];
    $username = trim($_POST['username']);
    $real_name = trim($_POST['real_name']);
    $phone = trim($_POST['phone'] ?? '');
    $phone = $phone !== '' ? $phone : null;
    $role = $_POST['role'];
    $group_id = $_POST['group_id'] ?: null;
    $status = $_POST['status'] ?? 'active';
    $resignation_date = !empty($_POST['resignation_date']) ? $_POST['resignation_date'] : null;
    $managed_group_ids = trim($_POST['managed_group_ids'] ?? '');
    $login_prompt_enabled = bool_post('login_prompt_enabled');
    $login_prompt_text = trim($_POST['login_prompt_text'] ?? '');
    $checkin_reminder_enabled = bool_post('checkin_reminder_enabled');
    $sector = trim($_POST['sector'] ?? '');
    $skip_newbie = isset($_POST['skip_newbie']) ? 1 : 0;

    $stmt = $pdo->prepare("SELECT * FROM users WHERE id = ? AND company_id = ?");
    $stmt->execute([$edit_id, $selected_company_id]);
    $old_data = $stmt->fetch();

    if (!$old_data) {
        $error = '用户不存在！';
    } elseif (!current_user_can_edit_target($is_super_admin, $user, $old_data) || !can_manage_group_id($is_super_admin, $user, $group_id ?: 0)) {
        $error = '只能编辑你可管理小组内的用户！';
    } else {
        if ($username === '') {
            $error = '用户名不能为空！';
        } else {
            // 用户名查重（全局唯一，排除自身）
            $uname_q = $pdo->prepare("SELECT id FROM users WHERE username = ?");
            $uname_q->execute([$username]);
            $uname_row = $uname_q->fetch();
            if ($uname_row && (int)$uname_row['id'] !== (int)$edit_id) {
                $error = "用户名 " . $username . " 已存在，请更换用户名！";
            } elseif ($phone !== '' && phone_exists_except($pdo, $phone, $edit_id)) {
                $error = "联系电话 " . $phone . " 已存在，请更换！";
            } else {
                $old_group = $old_data['group_id'];
                if ($old_group != $group_id) {
                    $stmt = $pdo->prepare("UPDATE users SET username = ?, real_name = ?, phone = ?, role = ?, group_id = ?, status = ?, resignation_date = ?, managed_group_ids = ?, sector = ?, login_prompt_enabled = ?, login_prompt_text = ?, checkin_reminder_enabled = ?, skip_newbie = ?, group_id_set_by = ?, group_id_set_at = NOW() WHERE id = ? AND company_id = ?");
                    $stmt->execute([$username, $real_name, $phone, $role, $group_id, $status, $resignation_date, $managed_group_ids, $sector, $login_prompt_enabled, $login_prompt_text, $checkin_reminder_enabled, $skip_newbie, $user['id'], $edit_id, $selected_company_id]);
                    auto_collect_reserved_sectors($pdo, $sector);
                    $message = "用户信息已更新！小组ID已修改，记录人：" . $user['real_name'];
                } else {
                    $stmt = $pdo->prepare("UPDATE users SET username = ?, real_name = ?, phone = ?, role = ?, status = ?, resignation_date = ?, managed_group_ids = ?, sector = ?, login_prompt_enabled = ?, login_prompt_text = ?, checkin_reminder_enabled = ?, skip_newbie = ? WHERE id = ? AND company_id = ?");
                    $stmt->execute([$username, $real_name, $phone, $role, $status, $resignation_date, $managed_group_ids, $sector, $login_prompt_enabled, $login_prompt_text, $checkin_reminder_enabled, $skip_newbie, $edit_id, $selected_company_id]);
                    auto_collect_reserved_sectors($pdo, $sector);
                    $message = "用户信息已更新！";
                }

                // 编辑时若重新选择了权限组则套用；未选则保留现有权限。非超管仅能套用自己已持有的权限
                $edit_profile_choice = (int)($_POST['permission_profile'] ?? 0);
                if ($edit_profile_choice > 0) {
                    $pp = $pdo->prepare("SELECT * FROM permission_profiles WHERE id = ? AND company_id = ?");
                    $pp->execute([$edit_profile_choice, $selected_company_id]);
                    $profile_row = $pp->fetch();
                    if ($profile_row) {
                        $flat_profile = profile_row_to_flat($profile_row);
                        if (api_user_owns_profile_perms($user, $flat_profile)) {
                            apply_permission_profile_to_user($pdo, $edit_id, $selected_company_id, $flat_profile, $profile_row['id']);
                        }
                    }
                }

                if (!empty($_POST['password'])) {
                    $plain_password = $_POST['password'];
                    $new_password = password_hash($plain_password, PASSWORD_DEFAULT);
                    $stmt = $pdo->prepare("UPDATE users SET password = ?, plain_password = ? WHERE id = ? AND company_id = ?");
                    $stmt->execute([$new_password, $plain_password, $edit_id, $selected_company_id]);
                    $message .= " 密码已修改！";
                }

                log_operation($pdo, $user['id'], $user['username'], $user['real_name'], 'edit', 'user', $edit_id, $real_name, json_encode($old_data), json_encode(['username' => $username, 'real_name' => $real_name, 'role' => $role, 'group_id' => $group_id, 'status' => $status, 'resignation_date' => $resignation_date, 'managed_group_ids' => $managed_group_ids, 'login_prompt_enabled' => $login_prompt_enabled, 'login_prompt_text' => $login_prompt_text, 'checkin_reminder_enabled' => $checkin_reminder_enabled]), $selected_company_id);
            }
        }
    }
}

// ============================================================
// 切换用户状态
// ============================================================
if (isset($_GET['toggle_status']) && isset($_GET['user_id'])) {
    $toggle_id = intval($_GET['user_id']);
    $new_status = $_GET['toggle_status'];
    $is_ajax = isset($_GET['ajax']);

    if (!in_array($new_status, ['active', 'inactive'])) {
        $error = "无效的状态值";
    } elseif ($toggle_id == $user['id'] && !$is_super_admin) {
        $error = "不能修改自己的状态！";
    } else {
        $stmt = $pdo->prepare("SELECT real_name, status, group_id FROM users WHERE id = ? AND company_id = ?");
        $stmt->execute([$toggle_id, $selected_company_id]);
        $target_user = $stmt->fetch();

        if ($target_user && !current_user_can_edit_target($is_super_admin, $user, $target_user)) {
            $error = "只能操作你可管理小组内的用户！";
        } elseif ($target_user) {
            if ($new_status == 'inactive') {
                $stmt = $pdo->prepare("UPDATE users SET status = ?, resignation_date = CURDATE() WHERE id = ? AND company_id = ?");
                $stmt->execute([$new_status, $toggle_id, $selected_company_id]);
            } else {
                $stmt = $pdo->prepare("UPDATE users SET status = ?, resignation_date = NULL WHERE id = ? AND company_id = ?");
                $stmt->execute([$new_status, $toggle_id, $selected_company_id]);
            }
            $status_text = $new_status == 'active' ? '启用' : '禁用/离职';
            $message = "用户 {$target_user['real_name']} 已{$status_text}！";
            log_operation($pdo, $user['id'], $user['username'], $user['real_name'], 'toggle_status', 'user', $toggle_id, $target_user['real_name'], json_encode(['old_status' => $target_user['status']]), json_encode(['new_status' => $new_status]), $selected_company_id);
        } else {
            $error = "用户不存在！";
        }
    }

    if ($is_ajax) {
        header('Content-Type: application/json; charset=utf-8');
        echo json_encode(['success' => !isset($error), 'message' => $message ?: $error]);
        exit;
    }
}


// ============================================================
// 快速修改人员赛道（点击赛道列弹窗选择保存）
// ============================================================
if (isset($_GET['quick_sector']) && isset($_GET['user_id'])) {
    $qs_id = intval($_GET['user_id']);
    $qs_sector = trim($_GET['sector'] ?? '');
    $is_ajax = isset($_GET['ajax']);
    $message = null;
    $error = null;

    if ($qs_id == $user['id'] && !$is_super_admin) {
        $error = "不能修改自己的赛道！";
    } else {
        $stmt = $pdo->prepare("SELECT real_name, status, group_id, sector FROM users WHERE id = ? AND company_id = ?");
        $stmt->execute([$qs_id, $selected_company_id]);
        $target_u = $stmt->fetch();

        if ($target_u && !current_user_can_edit_target($is_super_admin, $user, $target_u)) {
            $error = "只能操作你可管理小组内的用户！";
        } elseif ($target_u) {
            $old_sector = $target_u['sector'] ?? '';
            $pdo->prepare("UPDATE users SET sector = ? WHERE id = ? AND company_id = ?")
                ->execute([$qs_sector !== '' ? $qs_sector : null, $qs_id, $selected_company_id]);
            auto_collect_reserved_sectors($pdo, $qs_sector);
            $message = "已更新 {$target_u['real_name']} 的赛道：" . ($old_sector !== '' ? $old_sector : '无') . " → " . ($qs_sector !== '' ? $qs_sector : '无');
            log_operation($pdo, $user['id'], $user['username'], $user['real_name'], 'update_sector', 'user', $qs_id, $target_u['real_name'], json_encode(['old_sector' => $old_sector]), json_encode(['new_sector' => $qs_sector]), $selected_company_id);
        } else {
            $error = "用户不存在！";
        }
    }

    if ($is_ajax) {
        header('Content-Type: application/json; charset=utf-8');
        echo json_encode(['success' => !isset($error), 'message' => $message ?: $error, 'sector' => $qs_sector]);
        exit;
    }
}
// ============================================================
// 系统预设赛道管理接口（添加 / 删除）
// ============================================================
if (isset($_GET['rs_action']) && can_user_manage_users_by_row($user)) {
    header('Content-Type: application/json; charset=utf-8');
    $rs_action = $_GET['rs_action'];
    $rs_name = trim($_GET['name'] ?? '');

    if ($rs_action === 'add' && $rs_name !== '') {
        if (in_array($rs_name, $system_sector_presets, true)) {
            echo json_encode(['success' => false, 'message' => '该赛道已存在，无需添加']); exit;
        }
        auto_collect_reserved_sectors($pdo, $rs_name);
        $new_list = array_column($pdo->query("SELECT name FROM sector_presets")->fetchAll(), 'name');
        echo json_encode(['success' => true, 'message' => '已添加系统预设：' . $rs_name, 'presets' => $new_list]); exit;
    }

    if ($rs_action === 'del' && $rs_name !== '') {
        $pdo->prepare("DELETE FROM sector_presets WHERE name = ?")->execute([$rs_name]);
        $new_list = array_column($pdo->query("SELECT name FROM sector_presets")->fetchAll(), 'name');
        echo json_encode(['success' => true, 'message' => '已删除系统预设：' . $rs_name, 'presets' => $new_list]); exit;
    }

    echo json_encode(['success' => false, 'message' => '无效操作']); exit;
}
// ============================================================
// 删除用户（带完整日志）
// ============================================================
if (isset($_GET['delete'])) {
    $delete_id = $_GET['delete'];
    $is_ajax = isset($_GET['ajax']);
    
    // 日志文件路径
    $log_dir = __DIR__ . '/logs/';
    if (!is_dir($log_dir)) {
        mkdir($log_dir, 0755, true);
    }
    $log_file = $log_dir . 'delete_user.log';
    
    // 写入日志函数
    function write_delete_log($log_file, $message, $data = null) {
        $timestamp = date('Y-m-d H:i:s');
        $ip = $_SERVER['REMOTE_ADDR'] ?? 'unknown';
        $log_entry = "[{$timestamp}] [IP: {$ip}] {$message}";
        if ($data !== null) {
            $log_entry .= " | Data: " . print_r($data, true);
        }
        $log_entry .= PHP_EOL;
        @file_put_contents($log_file, $log_entry, FILE_APPEND);
    }
    
    write_delete_log($log_file, "========== 开始删除用户 ==========");
    write_delete_log($log_file, "delete_id: {$delete_id}");
    write_delete_log($log_file, "当前用户: ID={$user['id']}, real_name={$user['real_name']}, role={$user['role']}");
    
    if ($delete_id == $user['id'] && !$is_super_admin) {
        $error = "不能删除自己的账号！";
        write_delete_log($log_file, "错误: 不能删除自己的账号！");
    } else {
        // 查询要删除的用户信息
        $stmt = $pdo->prepare("SELECT real_name, username, role, group_id FROM users WHERE id = ? AND company_id = ?");
        $stmt->execute([$delete_id, $selected_company_id]);
        $del_user = $stmt->fetch();
        write_delete_log($log_file, "查询用户结果: " . json_encode($del_user));
        
        if ($del_user && !current_user_can_edit_target($is_super_admin, $user, $del_user)) {
            $error = "只能删除你可管理小组内的用户！";
            write_delete_log($log_file, "错误: 越权删除小组外用户");
        } elseif ($del_user) {
            try {
                // 1. 先查询该用户有多少个账号
                $stmt = $pdo->prepare("SELECT COUNT(*) as count FROM tiktok_accounts WHERE owner_id = ? AND company_id = ?");
                $stmt->execute([$delete_id, $selected_company_id]);
                $account_count = $stmt->fetch()['count'];
                write_delete_log($log_file, "该用户拥有的账号数量: {$account_count}");
                
                // 2. 解除账号关联（将 owner_id 设为 NULL）
                $stmt = $pdo->prepare("UPDATE tiktok_accounts SET owner_id = NULL WHERE owner_id = ? AND company_id = ?");
                $result = $stmt->execute([$delete_id, $selected_company_id]);
                $affected_rows = $stmt->rowCount();
                write_delete_log($log_file, "解除账号关联: 影响了 {$affected_rows} 条记录 (执行结果: " . ($result ? '成功' : '失败') . ")");
                
                // 3. 转移收益/提现记录中的提交人关联，保留历史数据，避免外键阻止删除用户
                // submitter_id 字段不允许为 NULL，因此转移给当前执行删除的管理员；
                // 如果当前用户就是被删除用户，则转移给同公司其它管理员/用户。
                $replacement_submitter_id = intval($user['id']);
                if ($replacement_submitter_id === intval($delete_id)) {
                    $stmt = $pdo->prepare("
                        SELECT id FROM users
                        WHERE company_id = ? AND id <> ?
                        ORDER BY is_super_admin DESC, FIELD(role, 'manager', 'admin', 'super_admin', 'hr', 'supervisor', 'leader', 'member', 'user'), id ASC
                        LIMIT 1
                    ");
                    $stmt->execute([$selected_company_id, $delete_id]);
                    $replacement_submitter_id = intval($stmt->fetchColumn());
                }

                $stmt = $pdo->prepare("SELECT COUNT(*) FROM daily_income WHERE submitter_id = ? AND company_id = ?");
                $stmt->execute([$delete_id, $selected_company_id]);
                $income_ref_count = intval($stmt->fetchColumn());

                $withdraw_ref_count = 0;
                try {
                    $stmt = $pdo->prepare("SELECT COUNT(*) FROM withdrawal_records WHERE submitter_id = ? AND company_id = ?");
                    $stmt->execute([$delete_id, $selected_company_id]);
                    $withdraw_ref_count = intval($stmt->fetchColumn());
                } catch (Exception $e) {
                    write_delete_log($log_file, "查询提现提交人关联跳过/失败: " . $e->getMessage());
                }

                if (($income_ref_count > 0 || $withdraw_ref_count > 0) && $replacement_submitter_id <= 0) {
                    throw new Exception("该用户存在收益/提现历史记录，但当前公司没有可接收历史记录的其它用户，不能删除。请先新增或保留一个管理员用户。");
                }

                $stmt = $pdo->prepare("UPDATE daily_income SET submitter_id = ? WHERE submitter_id = ? AND company_id = ?");
                $result = $stmt->execute([$replacement_submitter_id, $delete_id, $selected_company_id]);
                $income_rows = $stmt->rowCount();
                write_delete_log($log_file, "转移收益提交人关联到用户ID {$replacement_submitter_id}: 影响了 {$income_rows} 条记录 (执行结果: " . ($result ? '成功' : '失败') . ")");

                try {
                    $stmt = $pdo->prepare("UPDATE withdrawal_records SET submitter_id = ? WHERE submitter_id = ? AND company_id = ?");
                    $result = $stmt->execute([$replacement_submitter_id, $delete_id, $selected_company_id]);
                    $withdraw_rows = $stmt->rowCount();
                    write_delete_log($log_file, "转移提现提交人关联到用户ID {$replacement_submitter_id}: 影响了 {$withdraw_rows} 条记录 (执行结果: " . ($result ? '成功' : '失败') . ")");
                } catch (Exception $e) {
                    write_delete_log($log_file, "转移提现提交人关联跳过/失败: " . $e->getMessage());
                }

                // 4. 删除用户
                $stmt = $pdo->prepare("DELETE FROM users WHERE id = ? AND company_id = ?");
                $result = $stmt->execute([$delete_id, $selected_company_id]);
                $del_rows = $stmt->rowCount();
                write_delete_log($log_file, "删除用户: 删除了 {$del_rows} 条记录 (执行结果: " . ($result ? '成功' : '失败') . ")");
                
                if ($result && $del_rows > 0) {
                    // 5. 记录操作日志
                    write_delete_log($log_file, "开始记录操作日志...");
                    try {
                        log_operation($pdo, $user['id'], $user['username'], $user['real_name'], 'delete', 'user', $delete_id, $del_user['real_name'], json_encode($del_user), null, $selected_company_id);
                        write_delete_log($log_file, "操作日志记录成功");
                    } catch (Exception $e) {
                        write_delete_log($log_file, "操作日志记录失败: " . $e->getMessage());
                    }
                    
                    $message = "用户 {$del_user['real_name']} 已删除！该用户的 {$account_count} 个账号已解除关联，{$income_rows} 条收益记录已保留并转移给当前管理员。";
                    write_delete_log($log_file, "成功: " . $message);
                } else {
                    $error = "删除用户失败！";
                    write_delete_log($log_file, "错误: 删除用户失败，影响行数: {$del_rows}");
                }
                
            } catch (Exception $e) {
                $error = "删除失败：" . $e->getMessage();
                write_delete_log($log_file, "异常: " . $e->getMessage());
                write_delete_log($log_file, "异常堆栈: " . $e->getTraceAsString());
            }
        } else {
            $error = "用户不存在！";
            write_delete_log($log_file, "错误: 用户不存在");
        }
    }
    write_delete_log($log_file, "========== 删除用户结束 ==========");
    write_delete_log($log_file, "");

    if ($is_ajax) {
        header('Content-Type: application/json; charset=utf-8');
        echo json_encode(['success' => !isset($error), 'message' => $message ?: $error]);
        exit;
    }
}

// ============================================================
// ============================================================
// 权限组管理（CRUD）
// ============================================================
$permission_profiles = [];
// 添加员工时权限组下拉默认选中的权限组：优先「组员」/「员工」
$default_profile_id = 0;
if (can_user_manage_users_by_row($user)) {
    // 权限组为空时自动创建默认权限组（组员/人事/经理）
    ensure_default_permission_profiles($pdo, $selected_company_id);
    $permission_profiles = load_permission_profiles($pdo, $selected_company_id);
    foreach ($permission_profiles as $pp) {
        if (in_array(trim($pp['name']), ['组员', '员工'], true)) { $default_profile_id = (int)$pp['id']; break; }
    }
    $profiles_json = json_encode(array_map(function($p){
        return [
            'id' => (int)$p['id'],
            'name' => $p['name'],
            'company_id' => (int)$p['company_id'],
            'perms' => profile_row_to_flat($p)
        ];
    }, $permission_profiles), JSON_UNESCAPED_UNICODE);

    if ($_SERVER['REQUEST_METHOD'] == 'POST' && isset($_POST['action'])) {
        if ($_POST['action'] == 'profile_save') {
            if (!can_user_manage_users_by_row($user)) {
                $error = "没有权限管理权限组！";
            } else {
                $pid = (int)($_POST['profile_id'] ?? 0);
                $name = trim($_POST['profile_name'] ?? '');
                $desc = trim($_POST['profile_description'] ?? '');
                if ($name === '') {
                    $error = "权限组名称不能为空！";
                } else {
                    $values = [];
                    foreach (permission_profile_fields() as $col => $lbl) {
                        $values[$col] = bool_post('pp_' . $col);
                    }
                    // 仅允许管理当前公司的组
                    if ($pid > 0) {
                        $exist = $pdo->prepare("SELECT * FROM permission_profiles WHERE id = ? AND company_id = ?");
                        $exist->execute([$pid, $selected_company_id]);
                        if (!$exist->fetch()) {
                            $error = "权限组不存在或无权限修改！";
                        }
                    }
                    if (!$error) {
                        foreach ($values as $col => $v) {
                            if ($v === 1 && !api_can_grant_permission($user, $col)) {
                                $error = "你尚未拥有「" . api_permission_label($col) . "」权限，不能把它放进权限组！";
                                break;
                            }
                        }
                    }
                    if (!$error) {
                        insert_or_update_permission_profile($pdo, $selected_company_id, $pid, $name, $desc, $values);
                        $message = $pid ? "权限组「{$name}」更新成功！" : "权限组「{$name}」创建成功！";
                        log_operation($pdo, $user['id'], $user['username'], $user['real_name'], 'profile_save', 'user', $pid, '权限组', $name, json_encode(['profile_id' => $pid, 'name' => $name]), $selected_company_id);
                        // 权限组权限变更时，自动同步所有关联该组的员工权限
                        if ($pid > 0) {
                            ensure_user_permission_columns($pdo);
                            $linked = $pdo->prepare("SELECT id, real_name FROM users WHERE permission_profile_id = ? AND company_id = ?");
                            $linked->execute([$pid, $selected_company_id]);
                            $synced = 0;
                            foreach ($linked->fetchAll() as $lu) {
                                if (apply_permission_profile_to_user($pdo, $lu['id'], $selected_company_id, $values, $pid)) {
                                    $synced++;
                                }
                            }
                            if ($synced > 0) $message .= " 已同步 {$synced} 名关联员工权限。";
                        }
                        $permission_profiles = load_permission_profiles($pdo, $selected_company_id);
                        $profiles_json = json_encode(array_map(function($p){
                            return ['id'=>(int)$p['id'],'name'=>$p['name'],'company_id'=>(int)$p['company_id'],'perms'=>profile_row_to_flat($p)];
                        }, $permission_profiles), JSON_UNESCAPED_UNICODE);
                    }
                }
            }
        } elseif ($_POST['action'] == 'profile_delete') {
            $pid = (int)($_POST['profile_id'] ?? 0);
            $exist = $pdo->prepare("SELECT * FROM permission_profiles WHERE id = ? AND company_id = ?");
            $exist->execute([$pid, $selected_company_id]);
            $row = $exist->fetch();
            if (!$row) {
                $error = "权限组不存在或无权限删除！";
            } else {
                // 解除所有关联该权限组的员工（保留其当前权限快照）
                $pdo->prepare("UPDATE users SET permission_profile_id = NULL WHERE permission_profile_id = ? AND company_id = ?")
                    ->execute([$pid, $selected_company_id]);
                $pdo->prepare("DELETE FROM permission_profiles WHERE id = ? AND company_id = ?")->execute([$pid, $selected_company_id]);
                $message = "权限组「{$row['name']}」已删除！";
                log_operation($pdo, $user['id'], $user['username'], $user['real_name'], 'profile_delete', 'user', $pid, '权限组', $row['name'], null, $selected_company_id);
                $permission_profiles = load_permission_profiles($pdo, $selected_company_id);
                $profiles_json = json_encode(array_map(function($p){
                        return ['id'=>(int)$p['id'],'name'=>$p['name'],'company_id'=>(int)$p['company_id'],'perms'=>profile_row_to_flat($p)];
                    }, $permission_profiles), JSON_UNESCAPED_UNICODE);
                }
            }
        } elseif ($_POST['action'] == 'profile_apply_to_user') {
            $pid = (int)($_POST['profile_id'] ?? 0);
            $uid = (int)($_POST['target_user_id'] ?? 0);
            $profile = null;
            foreach ($permission_profiles as $p) {
                if ((int)$p['id'] === $pid) { $profile = $p; break; }
            }
            if (!$profile) {
                $error = "权限组不存在！";
            } elseif (!$uid) {
                $error = "请选择要应用的员工！";
            } else {
                $target = $pdo->prepare("SELECT * FROM users WHERE id = ? AND company_id = ?");
                $target->execute([$uid, $selected_company_id]);
                $trow = $target->fetch();
                if (!$trow) {
                    $error = "员工不存在！";
                } elseif (!can_user_manage_users_by_row($user) && !current_user_can_edit_target($is_super_admin, $user, $trow)) {
                    $error = "没有权限应用权限组给该员工！";
                } elseif (!$user['is_super_admin'] && $uid === (int)$user['id']) {
                    $error = "不能把权限组套用到自己账号！";
                } elseif (!api_user_owns_profile_perms($user, profile_row_to_flat($profile))) {
                    $error = "你尚未拥有该权限组中的某些权限，不能套用！";
                } else {
                    // 确保 users 表字段齐全，避免因缺失字段导致更新失败
                    ensure_user_permission_columns($pdo);
                    $ok = apply_permission_profile_to_user($pdo, $uid, $selected_company_id, profile_row_to_flat($profile), $pid);
                    if (!$ok) {
                        $error = "权限组应用失败，请刷新页面后重试";
                    } else {
                        $message = "已将权限组「{$profile['name']}」应用到 " . htmlspecialchars($trow['real_name']) . "！";
                        log_operation($pdo, $user['id'], $user['username'], $user['real_name'], 'profile_apply', 'user', $uid, '权限组', $profile['name'], json_encode(['target' => $trow['real_name'], 'profile' => $profile['name']]), $selected_company_id);
                    }
                }
            }
        } elseif ($_POST['action'] == 'profile_unlink') {
            $uid = (int)($_POST['target_user_id'] ?? 0);
            if (!$uid || !can_user_manage_users_by_row($user)) {
                $error = "没有权限解除关联！";
            } else {
                $pdo->prepare("UPDATE users SET permission_profile_id = NULL WHERE id = ? AND company_id = ?")
                    ->execute([$uid, $selected_company_id]);
                $message = "已解除该员工的权限组关联（保留当前权限）。";
            }
        }
    } else {
        $profiles_json = '[]';
}

// AJAX 请求：仅返回 JSON 结果，不渲染整页（用于权限组套用等原地操作）
if (isset($_SERVER['HTTP_X_REQUESTED_WITH']) && strtolower($_SERVER['HTTP_X_REQUESTED_WITH']) === 'xmlhttprequest') {
    header('Content-Type: application/json; charset=utf-8');
    if (!empty($error)) {
        echo json_encode(['ok' => false, 'msg' => $error], JSON_UNESCAPED_UNICODE);
    } elseif (!empty($message)) {
        echo json_encode(['ok' => true, 'msg' => $message], JSON_UNESCAPED_UNICODE);
    } else {
        echo json_encode(['ok' => true, 'msg' => '操作成功'], JSON_UNESCAPED_UNICODE);
    }
    exit;
}

// ============================================================
// 统计信息
// ============================================================
$stats = $pdo->prepare("
    SELECT COUNT(*) as total,
           SUM(CASE WHEN role = 'manager' THEN 1 ELSE 0 END) as manager_count,
           SUM(CASE WHEN role = 'hr' THEN 1 ELSE 0 END) as hr_count,
           SUM(CASE WHEN role = 'supervisor' THEN 1 ELSE 0 END) as supervisor_count,
           SUM(CASE WHEN role = 'leader' THEN 1 ELSE 0 END) as leader_count,
           SUM(CASE WHEN role = 'member' THEN 1 ELSE 0 END) as member_count,
           COUNT(DISTINCT group_id) as group_count,
           SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END) as active_count,
           SUM(CASE WHEN status = 'inactive' THEN 1 ELSE 0 END) as inactive_count
    FROM users WHERE company_id = ? AND is_super_admin = 0
    " . (!empty($scope_group_ids) ? " AND group_id IN (" . implode(',', array_fill(0, count($scope_group_ids), '?')) . ")" : "") . "
");
$stats->execute(array_merge([$selected_company_id], $scope_group_ids));
$stats = $stats->fetch();

// ============================================================
// 获取用户列表
// ============================================================
$sql = "
    SELECT u.*, s.real_name as set_by_name, leader.real_name as leader_name
    FROM users u
    LEFT JOIN users s ON s.id = u.group_id_set_by
    LEFT JOIN users leader ON leader.group_id = u.group_id AND leader.role = 'leader' AND leader.company_id = u.company_id
    WHERE u.company_id = ? AND u.is_super_admin = 0
";
$params = [$selected_company_id];

if ($status_filter === 'active') {
    $sql .= " AND u.status = 'active'";
} elseif ($status_filter === 'inactive') {
    $sql .= " AND u.status = 'inactive'";
}
if (!empty($scope_group_ids)) {
    $sql .= " AND u.group_id IN (" . implode(',', array_fill(0, count($scope_group_ids), '?')) . ")";
    $params = array_merge($params, $scope_group_ids);
}

$sql .= " ORDER BY FIELD(u.role, 'manager', 'hr', 'supervisor', 'leader', 'member'), u.group_id, u.real_name";

$users_list = $pdo->prepare($sql);
$users_list->execute($params);
$users_list = $users_list->fetchAll();

$grouped_users = ['manager' => [], 'hr' => [], 'supervisor' => [], 'leader' => [], 'member' => []];
foreach ($users_list as $u) {
    $grouped_users[$u['role']][] = $u;
}

$edit_user = null;
if (isset($_GET['edit'])) {
    $stmt = $pdo->prepare("SELECT * FROM users WHERE id = ? AND company_id = ?");
    $stmt->execute([$_GET['edit'], $selected_company_id]);
    $edit_user = $stmt->fetch();
}
$modal_users = [];
foreach ($users_list as $u) {
    $ppid = isset($u['permission_profile_id']) ? (int)$u['permission_profile_id'] : 0;
    // 若数据库未关联权限组，尝试按当前权限字段反向匹配权限组（与列表页 inline 下拉保持一致）
    if ($ppid <= 0 && !empty($permission_profiles)) {
        $flat = profile_row_to_flat(array_merge(
            array_fill_keys(array_keys(permission_profile_fields()), 0),
            array_intersect_key($u, permission_profile_fields())
        ));
        foreach ($permission_profiles as $pp) {
            if (profile_row_to_flat($pp) === $flat) { $ppid = (int)$pp['id']; break; }
        }
    }
    $modal_users[$u['id']] = [
        'id' => (int)$u['id'],
        'username' => $u['username'] ?? '',
        'real_name' => $u['real_name'] ?? '',
        'phone' => $u['phone'] ?? '',
        'role' => $u['role'] ?? 'member',
        'group_id' => $u['group_id'] ?? '',
        'status' => $u['status'] ?? 'active',
        'resignation_date' => $u['resignation_date'] ?? '',
        'allow_view_revenue' => (int)($u['allow_view_revenue'] ?? 0),
        'allow_view_password' => (int)($u['allow_view_password'] ?? 0),
        'allow_manage_users' => (int)($u['allow_manage_users'] ?? 0),
        'managed_group_ids' => $u['managed_group_ids'] ?? '',
        'login_prompt_enabled' => (int)($u['login_prompt_enabled'] ?? 0),
        'login_prompt_text' => $u['login_prompt_text'] ?? '',
        'sector' => $u['sector'] ?? '',
        'skip_newbie' => (int)($u['skip_newbie'] ?? 0),
        'checkin_reminder_enabled' => (int)($u['checkin_reminder_enabled'] ?? 0),
        'permission_profile_id' => $ppid
    ];
}

// 获取选中公司的名称
$selected_company_name = '';
if ($is_super_admin && $selected_company_id) {
    $stmt = $pdo->prepare("SELECT company_name FROM companies WHERE id = ?");
    $stmt->execute([$selected_company_id]);
    $comp = $stmt->fetch();
    $selected_company_name = $comp ? $comp['company_name'] : '';
}

// ============================================================
// 预留赛道库（全局共享）
// ============================================================
// 系统预设赛道（存于 sector_presets 表，可增删）
// ============================================================
$system_sector_presets = [];
try {
    $stmt = $pdo->query("SELECT name FROM sector_presets ORDER BY id");
    $system_sector_presets = array_column($stmt->fetchAll(), 'name');
} catch (Exception $e) {
    $system_sector_presets = [];
}

// 保存赛道时，自动把填写过的新赛道收录为系统预设
function auto_collect_reserved_sectors($pdo, $sector_str) {
    if (empty($sector_str)) return;
    $existing = [];
    try {
        $existing = array_column($pdo->query("SELECT name FROM sector_presets")->fetchAll(), 'name');
    } catch (Exception $e) {}
    $j = [];
    foreach (explode(',', (string)$sector_str) as $s) {
        $s = trim($s);
        if ($s !== '' && !in_array($s, $existing, true)) $j[] = $s;
    }
    if (!$j) return;
    try {
        $pdo->prepare("INSERT IGNORE INTO sector_presets (name) VALUES (?)")
           ->execute(array_map(function($s){ return $s; }, $j));
    } catch (Exception $e) {}
}

// 渲染赛道字段：统一显示
function render_sector_display($sector_str) {
    if (empty($sector_str)) return '-';
    $parts = array_map('trim', explode(',', (string)$sector_str));
    $html = [];
    foreach ($parts as $p) {
        if ($p === '') continue;
        $html[] = '<span class="sector-item">' . htmlspecialchars($p) . '</span>';
    }
    return implode('', $html);
}
?>
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>用户管理 - <?php echo $system_logo; ?></title>
    <style>
        .user-avatar { width:28px; height:28px; border-radius:50%; object-fit:cover; border:1px solid #ddd; flex-shrink:0; }
        .user-avatar-fallback { width:28px; height:28px; display:inline-flex; align-items:center; justify-content:center; font-size:16px; flex-shrink:0; }
        * { margin:0; padding:0; box-sizing:border-box; }
        body { background:#f5f7fb; font-family:-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; }
        .header { background:white; box-shadow:0 1px 4px rgba(0,0,0,0.1); padding:16px 24px; display:flex; justify-content:space-between; align-items:center; }
        .logo { font-size:20px; font-weight:bold; color:#667eea; }
        .main-wrapper { max-width:auto; margin:0 auto; padding:0 24px; }
        .nav-tabs { display:flex; gap:10px; margin-bottom:24px; background:white; padding:8px 16px; border-radius:12px; flex-wrap:wrap; }
        .nav-tabs a { padding:10px 20px; text-decoration:none; color:#666; border-radius:8px; }
        .nav-tabs a.active, .nav-tabs a:hover { background:#667eea; color:white; }
        .container { width:100%; }
        .card { background:white; border-radius:16px; padding:24px; margin-bottom:24px; box-shadow:0 1px 3px rgba(0,0,0,0.1); }
        .card h2 { margin-bottom:20px; color:#333; font-size:18px; border-left:3px solid #667eea; padding-left:12px; }
        .form-group { margin-bottom:15px; }
        .form-group label { display:block; margin-bottom:5px; font-weight:500; font-size:14px; }
        .form-group input, .form-group select { width:100%; padding:10px; border:1px solid #ddd; border-radius:8px; font-size:14px; }
        .form-row { display:grid; grid-template-columns:repeat(auto-fit,minmax(200px,1fr)); gap:15px; }
        .btn-submit { background:#667eea; color:white; border:none; padding:10px 20px; border-radius:8px; cursor:pointer; font-size:14px; }
        .pp-sel { padding:6px 8px; border:1px solid #ddd; border-radius:8px; font-size:13px; min-width:130px; }
        .pp-no-perm { color:#bbb; font-size:12px; }
        .btn-cancel { background:#f0f0f0; color:#666; border:none; padding:10px 20px; border-radius:8px; cursor:pointer; text-decoration:none; display:inline-block; font-size:14px; }
        .btn-status { background:#ff9800; color:white; border:none; padding:4px 12px; border-radius:6px; cursor:pointer; font-size:12px; margin:0 2px; display:inline-block; text-decoration:none; }
        .btn-status-active { background:#4caf50; }
        .btn-status-inactive { background:#f44336; }
        .stats-grid { display:grid; grid-template-columns:repeat(auto-fit,minmax(150px,1fr)); gap:15px; margin-bottom:20px; }
        .stat-card { background:linear-gradient(135deg,#667eea 0%,#764ba2 100%); color:white; border-radius:12px; padding:15px; text-align:center; }
        .stat-card .number { font-size:28px; font-weight:bold; }
        .stat-card .label { font-size:12px; opacity:0.9; margin-top:5px; }
        .role-section { margin-bottom:30px; }
        .role-header { display:flex; align-items:center; gap:10px; margin-bottom:15px; padding-bottom:10px; border-bottom:2px solid #eee; }
        .role-icon { font-size:24px; }
        .role-title { font-size:18px; font-weight:bold; }
        .role-count { background:#f0f0f0; color:#666; padding:2px 10px; border-radius:20px; font-size:12px; }
        table { width:100%; border-collapse:collapse; }
        th, td { padding:12px 10px; text-align:left; border-bottom:1px solid #eee; }
        th { background:#f8f9fa; color:#555; font-weight:600; font-size:13px; }
        td { font-size:14px; }
        /* 各模块表格列宽统一，保证列上下对齐 */
        .col-pw { width:110px; }
        .col-name { width:180px; }
        .col-phone { width:130px; }
        .col-role { width:90px; }
        .col-sector { width:120px; }
        .sector-item { display:inline-block; margin-right:4px; }
        .col-profile { width:160px; }
        .col-status { width:100px; }
        .col-time { width:110px; }
        .col-op { width:200px; }
        .password-cell { font-family: monospace; font-size: 12px; }
        .perm-badge { display:inline-block; padding:3px 8px; border-radius:12px; font-size:11px; font-weight:600; margin:2px; white-space:nowrap; }
        .perm-on { background:#e8f5e9; color:#2e7d32; }
        .perm-off { background:#f5f5f5; color:#999; }
        span.perm-clickable { cursor:pointer; transition:all 0.2s; border:1px solid transparent; user-select:none; }
        span.perm-clickable:hover { transform:scale(1.05); box-shadow:0 2px 4px rgba(0,0,0,0.15); }
        span.perm-clickable.perm-on:hover { background:#c8e6c9; border-color:#81c784; }
        span.perm-clickable.perm-off:hover { background:#e0e0e0; border-color:#bdbdbd; color:#666; }
        .perm-fold { position:relative; }
        .perm-fold .perm-badges { display:flex; flex-wrap:wrap; gap:2px; max-height:32px; overflow:hidden; transition:max-height .25s ease; }
        .perm-fold.expanded .perm-badges { max-height:400px; }
        .perm-fold-toggle { display:none; margin-top:2px; font-size:11px; color:#1976d2; cursor:pointer; user-select:none; }
        .perm-fold.expanded .pf-caret { transform:rotate(180deg); display:inline-block; }
        .perm-cell:not(.perm-cell-empty) .perm-fold-toggle { display:inline-block; }
        .perm-cell-empty { color:#bbb; }
        .toast {
            position: fixed; top: 20px; left: 50%;
            transform: translateX(-50%) translateY(-20px);
            padding: 10px 24px; border-radius: 8px;
            color: #fff; font-size: 14px; font-weight: 500;
            z-index: 9999; opacity: 0;
            transition: all 0.3s ease; pointer-events: none;
            box-shadow: 0 4px 12px rgba(0,0,0,0.15);
        }
        .toast-show { opacity: 1; transform: translateX(-50%) translateY(0); }
        .toast-success { background: #4caf50; }
        .toast-error { background: #f44336; }
        .toast-warning { background: #ff9800; }
        .permission-options { display:flex; flex-wrap:wrap; gap:12px; align-items:center; padding:10px 12px; border:1px solid #e5e7eb; border-radius:8px; background:#fafafa; }
        .permission-options label { display:flex; align-items:center; gap:6px; margin:0; font-weight:500; color:#444; cursor:pointer; }
        .permission-options input { width:auto; }
        .role-badge { display:inline-block; padding:4px 12px; border-radius:20px; font-size:12px; font-weight:500; }
        .role-manager { background:#f3e5f5; color:#9c27b0; }
        .role-hr { background:#efebe9; color:#5d4037; }
        .role-supervisor { background:#e3f2fd; color:#1976d2; }
        .role-leader { background:#e8f5e9; color:#388e3c; }
        .role-member { background:#fff3e0; color:#f57c00; }
        .status-badge { display:inline-block; padding:4px 12px; border-radius:20px; font-size:11px; font-weight:500; }
        .status-active { background:#e8f5e9; color:#388e3c; }
        .status-inactive { background:#ffebee; color:#c62828; }
        .message { background:#d4edda; color:#155724; padding:12px; border-radius:8px; margin-bottom:20px; }
        .error { background:#f8d7da; color:#721c24; padding:12px; border-radius:8px; margin-bottom:20px; }
        .btn-edit, .btn-delete { padding:4px 10px; border-radius:6px; text-decoration:none; font-size:12px; margin:0 2px; display:inline-block; }
        .btn-edit { background:#e3f2fd; color:#1976d2; }
        .btn-delete { background:#fee; color:#c33; }
        .table-actions { white-space:nowrap; }
        .ops-dd { position:relative; display:inline-block; }
        .ops-dd-btn { background:#6c5ce7; color:#fff; border:none; border-radius:6px; padding:6px 12px; cursor:pointer; font-size:13px; line-height:1; }
        .ops-dd-btn:hover { opacity:.85; }
        .ops-dd-caret { font-size:9px; opacity:.7; margin-left:2px; }
        .ops-dd-menu { display:none; position:absolute; right:0; top:calc(100% + 4px); background:#fff; border:1px solid #e0e0e0; border-radius:8px; box-shadow:0 4px 14px rgba(0,0,0,.12); min-width:112px; z-index:50; padding:4px; }
        .ops-dd.open .ops-dd-menu { display:block; }
        .ops-dd-item { display:block; text-decoration:none; color:#333; font-size:13px; padding:8px 10px; border-radius:6px; white-space:nowrap; }
        .ops-dd-item:hover { background:#f5f5f5; }
        .ops-dd-item.ops-dd-danger { color:#e74c3c; }
        .ops-dd-item.ops-dd-danger:hover { background:#fdecea; }
        /* 操作列改为平铺按钮，不再折叠 */
        .table-actions .ops-dd, .action-btns .ops-dd { display:flex; gap:5px; flex-wrap:wrap; position:static; }
        .table-actions .ops-dd-btn, .action-btns .ops-dd-btn,
        .table-actions .ops-dd-caret, .action-btns .ops-dd-caret { display:none !important; }
        .table-actions .ops-dd-menu, .action-btns .ops-dd-menu { display:inline-flex !important; position:static !important; background:transparent; border:none; box-shadow:none; padding:0; gap:5px; min-width:0; }
        .table-actions .ops-dd-item, .action-btns .ops-dd-item { display:inline-block; padding:5px 10px; border-radius:6px; background:#f0f2f5; color:#333; font-size:12px; line-height:1; white-space:normal; }
        .table-actions .ops-dd-item:hover, .action-btns .ops-dd-item:hover { background:#e4e7ec; }
        .table-actions .ops-dd-item.ops-dd-danger, .action-btns .ops-dd-item.ops-dd-danger { background:#fdecea; color:#e74c3c; }
        .table-actions .ops-dd-item.ops-dd-danger:hover, .action-btns .ops-dd-item.ops-dd-danger:hover { background:#f9d5d2; }
        .checkbox-col { width:38px; text-align:center; }
        .current-user-row { background:#f0f7ff; }
        .leader-name { color:#388e3c; font-weight:500; }
        .group-id-hint { font-size:11px; color:#888; display:block; }
        .group-badge { display:inline-block; background:#e8eaf6; color:#3949ab; padding:2px 8px; border-radius:12px; font-size:11px; margin-left:8px; }
        .filter-bar { background:#f8f9fa; padding:15px 20px; border-radius:12px; margin-bottom:20px; display:flex; flex-wrap:wrap; align-items:flex-end; gap:15px; }
        .filter-group { display:flex; flex-direction:column; gap:5px; }
        .filter-group label { font-size:12px; color:#888; }
        .filter-group select { padding:8px 12px; border:1px solid #ddd; border-radius:8px; font-size:13px; min-width:120px; }
        .company-selector {
            background: #f0f0f0;
            padding: 12px 20px;
            border-radius: 12px;
            margin-bottom: 20px;
            display: flex;
            align-items: center;
            gap: 15px;
            flex-wrap: wrap;
        }
        .company-selector label {
            font-weight: 500;
            color: #555;
        }
        .company-selector select {
            padding: 8px 12px;
            border: 1px solid #ddd;
            border-radius: 8px;
            font-size: 14px;
            background: white;
            min-width: 200px;
        }
        .company-selector button {
            background: #667eea;
            color: white;
            border: none;
            padding: 8px 16px;
            border-radius: 8px;
            cursor: pointer;
        }
        .page-actions { display:flex; justify-content:space-between; align-items:center; gap:12px; margin-bottom:20px; flex-wrap:wrap; }
        .page-actions .hint { color:#777; font-size:13px; }
        .modal-mask { position:fixed; inset:0; z-index:9999; display:none; align-items:center; justify-content:center; background:rgba(15,23,42,0.45); padding:24px; }
        .modal-mask.show { display:flex; }
        .user-modal { width:min(760px, 96vw); max-height:90vh; overflow:auto; background:white; border-radius:18px; box-shadow:0 24px 80px rgba(15,23,42,0.28); }
        .user-modal-header { display:flex; justify-content:space-between; align-items:center; padding:18px 22px; border-bottom:1px solid #edf0f5; }
        .user-modal-header h2 { margin:0; color:#333; font-size:18px; }
        .modal-close { width:34px; height:34px; border:0; border-radius:8px; background:#f3f4f6; color:#555; cursor:pointer; font-size:20px; line-height:1; }
        .modal-close:hover { background:#e5e7eb; }
        .user-modal-body { padding:22px; }
        .modal-actions { display:flex; justify-content:flex-end; gap:10px; margin-top:8px; }
        @media (max-width:768px) { .main-wrapper { padding:0 16px; } .nav-tabs a { padding:6px 12px; font-size:12px; } th,td { padding:8px; font-size:12px; } .stats-grid { grid-template-columns:repeat(3,1fr); } }
        /* 添加用户时权限组下拉锁定样式 */
        .profile-locked { background:#f3f4f6 !important; color:#666 !important; cursor:not-allowed !important; opacity:0.85; }
        .profile-locked:hover, .profile-locked:focus { border-color:#ddd !important; box-shadow:none !important; }
    </style>
</head>
<body>
<?php render_header($pdo, $user); ?>
<div class="main-wrapper">
    <?php include 'nav.php'; ?>
    <div class="container">
        <?php if($is_super_admin && count($companies) > 0): ?>
        <div class="company-selector">
            <label>🏢 切换公司：</label>
            <form method="GET" id="companyForm" style="display:flex; gap:10px; align-items:center;">
                <input type="hidden" name="status" value="<?php echo $status_filter; ?>">
                <select name="company_id" id="companySelect" onchange="this.form.submit()">
                    <?php foreach($companies as $comp): ?>
                    <option value="<?php echo $comp['id']; ?>" <?php echo ($selected_company_id == $comp['id']) ? 'selected' : ''; ?>>
                        <?php echo htmlspecialchars($comp['company_name']); ?>
                        <?php echo ($comp['status'] == 'expired') ? ' (已到期)' : ''; ?>
                    </option>
                    <?php endforeach; ?>
                </select>
            </form>
            <?php if($selected_company_id != $company_id): ?>
            <span style="color:#ff9800; font-size:12px;">⚠️ 当前正在管理其他公司的用户</span>
            <?php endif; ?>
        </div>
        <?php endif; ?>
        
        <?php if($message): ?><div id="pageMessage" data-msg="<?php echo htmlspecialchars($message); ?>" data-type="success" style="display:none;"></div><?php endif; ?>
        <?php if($error): ?><div id="pageMessage" data-msg="<?php echo htmlspecialchars($error); ?>" data-type="error" style="display:none;"></div><?php endif; ?>
        
        <!-- 统计卡片 -->
        <div class="stats-grid">
            <div class="stat-card"><div class="number"><?php echo $stats['total']; ?></div><div class="label">总人数</div></div>
            <div class="stat-card" style="background:linear-gradient(135deg,#4caf50,#2e7d32);"><div class="number"><?php echo $stats['active_count']; ?></div><div class="label">✅ 在职</div></div>
            <div class="stat-card" style="background:linear-gradient(135deg,#f44336,#c62828);"><div class="number"><?php echo $stats['inactive_count']; ?></div><div class="label">🚫 离职/禁用</div></div>
            <div class="stat-card" style="background:linear-gradient(135deg,#9c27b0,#6a1b9a);"><div class="number"><?php echo $stats['manager_count']; ?></div><div class="label">👑 经理</div></div>
            <div class="stat-card" style="background:linear-gradient(135deg,#1976d2,#0d47a1);"><div class="number"><?php echo $stats['supervisor_count']; ?></div><div class="label">👔 主管</div></div>
            <div class="stat-card" style="background:linear-gradient(135deg,#388e3c,#1b5e20);"><div class="number"><?php echo $stats['leader_count']; ?></div><div class="label">👥 组长</div></div>
            <div class="stat-card" style="background:linear-gradient(135deg,#f57c00,#e65100);"><div class="number"><?php echo $stats['member_count']; ?></div><div class="label">👤 组员</div></div>
            <div class="stat-card" style="background:linear-gradient(135deg,#607d8b,#37474f);"><div class="number"><?php echo $stats['group_count']; ?></div><div class="label">📁 小组数</div></div>
        </div>
        
        <!-- 状态筛选栏 -->
        <div class="filter-bar">
            <div class="filter-group">
                <label>📊 状态筛选</label>
                <select id="status_filter" onchange="window.location.href='users.php?status='+this.value<?php echo $is_super_admin ? '+&company_id=<?php echo $selected_company_id; ?>' : ''; ?>">
                    <option value="" <?php echo $status_filter == '' ? 'selected' : ''; ?>>全部用户</option>
                    <option value="active" <?php echo $status_filter == 'active' ? 'selected' : ''; ?>>✅ 在职</option>
                    <option value="inactive" <?php echo $status_filter == 'inactive' ? 'selected' : ''; ?>>🚫 离职/禁用</option>
                </select>
            </div>
            <div class="filter-group">
                <a href="users.php<?php echo $is_super_admin ? '?company_id=' . $selected_company_id : ''; ?>" class="btn-cancel" style="padding:8px 16px;">重置筛选</a>
            </div>
            <div class="filter-group">
                <button type="button" class="btn-submit" onclick="openUserModal('add')">➕ 添加新用户</button>
            </div>
        </div>
        
        <!-- 添加/编辑用户弹窗 -->
        <div class="modal-mask" id="userModalMask" onclick="closeUserModal(event)">
            <div class="user-modal" onclick="event.stopPropagation()">
                <div class="user-modal-header">
                    <h2 id="userModalTitle">➕ 添加新用户</h2>
                    <button type="button" class="modal-close" onclick="closeUserModal()">×</button>
                </div>
                <div class="user-modal-body">
            <form method="POST" id="userEditForm" action="users.php<?php echo $is_super_admin ? '?company_id=' . $selected_company_id : ''; ?>">
                <input type="hidden" name="action" id="modalAction" value="add">
                <?php if($is_super_admin): ?>
                <input type="hidden" name="company_id" value="<?php echo $selected_company_id; ?>">
                <?php endif; ?>
                <input type="hidden" name="user_id" id="modalUserId" value="">
                <div class="form-row">
                    <div class="form-group"><label>用户名(登录用)</label><input type="text" name="username" id="modalUsername" required oninput="checkUsernameDup()"><span id="usernameStatus" style="display:none;color:#e74c3c;font-size:12px;margin-top:2px;">该用户名已存在，请更改</span></div>
                    <div class="form-group"><label>真实姓名</label><input type="text" name="real_name" id="modalRealName" required></div>
                    <div class="form-group"><label>联系电话</label><input type="text" name="phone" id="modalPhone" placeholder="手机号/座机"></div>
                </div>
                <div class="form-row">
                    <div class="form-group"><label id="modalPasswordLabel">密码</label><input type="text" name="password" id="modalPassword" value="123456" placeholder="默认123456"></div>
                    <div class="form-group"><label>角色</label><select name="role" id="modalRole" onchange="applyRoleDefaultPermissions(this.value)"><option value="member">组员</option><option value="leader">组长</option><option value="supervisor">主管</option><option value="hr">人事</option><option value="manager">经理</option></select></div>
                    <div class="form-group"><label>小组ID</label><input type="number" name="group_id" id="modalGroupId" placeholder="1,2,3..."></div>
                </div>
                <div class="form-row">
                    <div class="form-group"><label>状态</label>
                        <select name="status" id="modalStatus">
                            <option value="active">✅ 在职</option>
                            <option value="inactive">🚫 离职/禁用</option>
                        </select>
                    </div>
                    <div class="form-group"><label>离职时间</label>
                        <input type="date" name="resignation_date" id="modalResignationDate">
                    </div>
                </div>
                <div class="form-group">
                    <label>赛道（视频解说方向，可多选用逗号分隔）</label>
                    <input type="text" name="sector" id="modalSector" placeholder="如：动漫,体育" style="width:100%;padding:10px;border:1px solid #ddd;border-radius:8px;font-size:14px;">
                    <div style="margin-top:8px;">
                        <div style="display:flex;gap:6px;flex-wrap:wrap;align-items:center;" id="sectorReservedOptions">
                            <?php foreach($system_sector_presets as $s): ?>
                            <button type="button" class="sector-chip" data-sector="<?php echo htmlspecialchars($s); ?>" onclick="toggleSectorChip(this)" style="padding:4px 10px;border:1px solid #ddd;border-radius:20px;background:white;color:#555;font-size:12px;cursor:pointer;"><?php echo htmlspecialchars($s); ?><span class="rs-del" onclick="event.stopPropagation();delReservedSector(this)" style="margin-left:5px;color:#bbb;cursor:pointer;">✕</span></button>
                            <?php endforeach; ?>
                            <button type="button" onclick="openReservedSectorAdd(this)" style="padding:4px 10px;border:1px dashed #bbb;border-radius:20px;background:#fff;color:#999;font-size:12px;cursor:pointer;">＋ 添加系统预设</button>
                        </div>
                    </div>
                </div>
                <div class="form-group">
                    <label style="display:flex;align-items:center;gap:8px;">
                        <input type="checkbox" name="skip_newbie" id="modalSkipNewbie" style="width:16px;height:16px;">
                        <span>标记为正式（跳过新手期）</span>
                        <span style="font-size:11px;color:#999;font-weight:normal;">不勾选=按创建15天自动判定新手</span>
                    </label>
                </div>
                <div class="form-group" id="perpGroupSelectorGroup">
                    <label>选择权限组（添加新用户后直接套用该权限组）</label>
                    <select name="permission_profile" id="modalPermissionProfile" onchange="applyPermissionProfileToModal(this.value)" style="width:100%;padding:10px;border:1px solid #ddd;border-radius:8px;font-size:14px;">
                        <?php foreach($permission_profiles as $pp): ?>
                        <option value="<?php echo (int)$pp['id']; ?>" <?php echo $default_profile_id === (int)$pp['id'] ? 'selected' : ''; ?>><?php echo htmlspecialchars($pp['name']); ?></option>
                        <?php endforeach; ?>
                    </select>
                    <span class="group-id-hint">添加新用户默认固定为「组员」权限组，不可修改；如需调整权限，请在该用户创建后编辑修改其权限组。</span>
                </div>

                <div class="form-group">
                    <label>可管理小组ID</label>
                    <input type="text" name="managed_group_ids" id="modalManagedGroupIds" placeholder="例如：1,2 表示可管理1组和2组">
                    <span class="group-id-hint">给主管或授权员工填写；留空时默认只能管理自己所在小组。经理/超管不受此限制。</span>
                </div>
                <div class="form-group">
                    <label>登录提示</label>
                    <div class="permission-options" style="margin-bottom:8px;">
                        <label><input type="checkbox" name="login_prompt_enabled" id="modalLoginPromptEnabled" value="1"> 启用登录提示（用户登录后显示提示内容）</label>
                        <label style="margin-left:24px;"><input type="checkbox" name="checkin_reminder_enabled" id="modalCheckinReminderEnabled" value="1"> 签到提醒（登录后显示钉钉考勤信息）</label>
                    </div>
                    <textarea name="login_prompt_text" id="modalLoginPromptText" rows="3" placeholder="输入登录后显示的提示内容，留空则不显示" style="width:100%;padding:10px;border:1px solid #ddd;border-radius:8px;font-size:14px;resize:vertical;"></textarea>
                    <span class="group-id-hint">开启后，该用户每次登录时会看到此提示内容。默认关闭。签到提醒需在钉钉考勤配置中启用（salary_admin.php）。</span>
                </div>
                <div class="modal-actions"><button type="button" class="btn-cancel" onclick="closeUserModal()">取消</button><button type="submit" class="btn-submit" id="modalSubmitBtn">添加用户</button></div>
            </form>
                </div>
            </div>
        </div>

        <!-- 权限组管理卡片 -->
        <?php if(api_can_manage($user)): ?>
        <div class="card" style="margin-top:20px;">
            <h2 style="display:flex;align-items:center;justify-content:space-between;">🎛️ 权限组管理
                <button type="button" class="btn-submit" onclick="openProfileModal()">➕ 新建权限组</button>
            </h2>
            <p style="color:#666;font-size:13px;margin-bottom:12px;">把一组权限打包成权限组，给员工选组一键套用（复制到员工自身权限，之后可再手动微调）。</p>
            <?php if(empty($permission_profiles)): ?>
                <div style="padding:20px;text-align:center;color:#999;background:#fafafa;border-radius:8px;">暂无权限组，点击上方「新建权限组」创建。</div>
            <?php else: ?>
            <div style="overflow-x:auto;">
                <table>
                    <thead>
                        <tr>
                            <th>权限组名称</th>
                            <th>覆盖权限</th>
                            <th>来源</th>
                            <th>操作</th>
                        </tr>
                    </thead>
                    <tbody>
                        <?php foreach($permission_profiles as $pp): $pp_flat = profile_row_to_flat($pp); $on = array_keys(array_filter($pp_flat)); ?>
                        <tr>
                            <td style="font-weight:bold;"><?php echo htmlspecialchars($pp['name']); ?><?php if(!empty($pp['description'])): ?><div style="font-weight:normal;color:#888;font-size:12px;"><?php echo htmlspecialchars($pp['description']); ?></div><?php endif; ?></td>
                            <td>
                                <?php
                                if(empty($on)) { echo '<span style="color:#bbb;">无任何权限</span>'; }
                                else {
                                    $labels = [];
                                    foreach ($on as $c) { $labels[] = $module_permissions[$c] ?? permission_profile_fields()[$c] ?? $c; }
                                    echo '<span style="font-size:12px;color:#555;">' . implode('、', array_slice($labels, 0, 6)) . (count($labels) > 6 ? ' 等' . count($labels) . '项' : '') . '</span>';
                                }
                                ?>
                            </td>
                            <td><span style="color:#1976d2;">本公司</span></td>
                            <td class="table-actions">
                                <div class="ops-dd">
                                    <button type="button" class="ops-dd-btn" onclick="toggleOpsMenu(event, this)">操作 <span class="ops-dd-caret">▾</span></button>
                                    <div class="ops-dd-menu">
                                        <a class="ops-dd-item" href="javascript:void(0)" onclick="openProfileModal(<?php echo (int)$pp['id']; ?>, <?php echo htmlspecialchars(json_encode($pp['name'])); ?>, <?php echo htmlspecialchars(json_encode($pp['description'] ?? '')); ?>, <?php echo htmlspecialchars(json_encode($pp_flat)); ?>, <?php echo (int)$pp['company_id']; ?>)">编辑</a>
                                        <?php if($pp['company_id'] != 0): ?>
                                        <a class="ops-dd-item ops-dd-danger" href="javascript:void(0)" onclick="deleteProfile(<?php echo (int)$pp['id']; ?>, <?php echo htmlspecialchars(json_encode($pp['name'])); ?>)">删除</a>
                                        <?php endif; ?>
                                    </div>
                                </div>
                            </td>
                        </tr>
                        <?php endforeach; ?>
                    </tbody>
                </table>
            </div>
            <?php endif; ?>
        </div>
        <?php endif; ?>

        <!-- 权限组编辑弹窗 -->
        <div class="modal-mask" id="profileModalMask" onclick="closeProfileModal(event)">
            <div class="user-modal" onclick="event.stopPropagation()">
                <div class="user-modal-header">
                    <h2 id="profileModalTitle">🎛️ 新建权限组</h2>
                    <button type="button" class="modal-close" onclick="closeProfileModal()">×</button>
                </div>
                <div class="user-modal-body">
                    <form method="POST" id="profileForm" action="users.php<?php echo $is_super_admin ? '?company_id=' . $selected_company_id : ''; ?>">
                        <input type="hidden" name="action" value="profile_save">
                        <?php if($is_super_admin): ?>
                        <input type="hidden" name="company_id" value="<?php echo $selected_company_id; ?>">
                        <?php endif; ?>
                        <input type="hidden" name="profile_id" id="profileModalId" value="0">
                        <div class="form-group"><label>权限组名称</label><input type="text" name="profile_name" id="profileModalName" required placeholder="如：行政专员、财务、剪辑师"></div>
                        <div class="form-group"><label>备注（可选）</label><input type="text" name="profile_description" id="profileModalDesc" placeholder="简要说明这个组的用途"></div>
                        <div class="form-group">
                            <label>组内权限（打包后供员工套用）</label>
                            <div style="display:flex;gap:6px;margin-bottom:8px;flex-wrap:wrap;">
                                <button type="button" class="btn-cancel" style="padding:5px 10px;font-size:12px;background:#27ae60;color:#fff;border:none;" onclick="setProfilePermChecks(true)">全选</button>
                                <button type="button" class="btn-cancel" style="padding:5px 10px;font-size:12px;" onclick="setProfilePermChecks(false)">清空</button>
                            </div>
                            <div class="permission-options">
                                <?php foreach(permission_profile_fields() as $pcol => $plabel): ?>
                                <label><input type="checkbox" name="pp_<?php echo htmlspecialchars($pcol); ?>" class="profile-perm-check" data-field="<?php echo htmlspecialchars($pcol); ?>" value="1"> <?php echo htmlspecialchars($plabel); ?></label>
                                <?php endforeach; ?>
                            </div>
                        </div>
                        <div class="modal-actions"><button type="button" class="btn-cancel" onclick="closeProfileModal()">取消</button><button type="submit" class="btn-submit" style="background:#6c5ce7;">保存权限组</button></div>
                    </form>
                </div>
            </div>
        </div>

        <!-- 经理区域 -->
        <?php if(!empty($grouped_users['manager'])): ?>
        <div class="role-section">
            <div class="role-header"><span class="role-icon">👑</span><span class="role-title">经理</span><span class="role-count"><?php echo count($grouped_users['manager']); ?>人</span></div>
            <div style="overflow-x:auto;">
                <table>
                    <thead>
                        <tr>
                            <th class="checkbox-col"><input type="checkbox" class="select-all-manager" onclick="toggleSelectByRole('manager', this)"></th>
                            <?php if($is_super_admin): ?><th class="col-pw">明文密码</th><?php endif; ?>
                            <th class="col-name">姓名</th>
                            <th class="col-phone">联系电话</th>
                            <th class="col-role">角色</th>
                            <th class="col-sector">赛道</th>
                            <th class="col-profile">权限组</th>
                            <th class="col-status">状态</th>
                            <th class="col-time">注册时间</th>
                            <th class="col-op">操作</th>
                        </tr>
                    </thead>
                    <tbody>
                        <?php foreach($grouped_users['manager'] as $u): $is_current=($u['id']==$user['id']); ?>
                        <tr class="<?php echo $is_current?'current-user-row':''; ?>">
                            <td class="checkbox-col"><?php if(!$is_current || $is_super_admin): ?><input type="checkbox" name="selected_users[]" value="<?php echo $u['id']; ?>" class="user-checkbox" data-role="manager"><?php endif; ?></td>
                            <?php if($is_super_admin): ?>
                            <td class="password-cell"><?php echo htmlspecialchars($u['plain_password'] ?? '-'); ?></td>
                            <?php endif; ?>
                            <td><div style="display:flex;flex-direction:column;align-items:flex-start;gap:1px;"><span style="font-size:10px;color:#888;line-height:1;"><?php echo htmlspecialchars($u['username']); ?></span><div style="display:flex;align-items:center;gap:6px;line-height:1.2;"><?php if(!empty($u['dingtalk_avatar'])): ?><img src="<?php echo htmlspecialchars($u['dingtalk_avatar']); ?>" class="user-avatar" alt="" onerror="this.style.display='none';this.nextElementSibling.style.display='inline'"><?php endif; ?><span class="user-avatar-fallback" style="<?php echo !empty($u['dingtalk_avatar']) ? 'display:none' : ''; ?>">👤</span><span><?php echo htmlspecialchars($u['real_name']); ?><?php $_nb_ts = strtotime((string)($u['created_at'] ?? '')); if(empty($u['skip_newbie']) && $_nb_ts && (time() - $_nb_ts) < 15*86400): ?><span style="font-size:10px;color:#fff;background:#f97316;border-radius:4px;padding:1px 5px;margin-left:5px;">新手</span><?php endif; ?><?php if($is_current): ?><span style="font-size:10px;color:#667eea;margin-left:5px;">(当前)</span><?php endif; ?></span></div></div></td>
                            <td style="font-size:13px;color:#555;"><?php echo htmlspecialchars($u['phone'] ?? ''); ?></td>
                            <td><span class="role-badge role-manager">经理</span></td>
                            <td class="sector-cell" data-id="<?php echo (int)$u['id']; ?>" data-sector="<?php echo $u['sector'] ? htmlspecialchars($u['sector']) : ''; ?>"><?php echo render_sector_display($u['sector']); ?></td>
                            <td><?php render_profile_select($u, $permission_profiles, $user, $is_super_admin, $selected_company_id); ?></td>
                            <td><span class="status-badge status-<?php echo $u['status']; ?>"><?php echo $u['status'] == 'active' ? '✅ 在职' : '🚫 离职'; ?></span></td>
                            <td><?php echo substr($u['created_at'],0,10); ?></td>
                            <td class="table-actions">
                                <div class="ops-dd">
                                    <button type="button" class="ops-dd-btn" onclick="toggleOpsMenu(event, this)">操作 <span class="ops-dd-caret">▾</span></button>
                                    <div class="ops-dd-menu">
                                        <a class="ops-dd-item" href="?edit=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>">编辑</a>
                                        <?php if($u['status'] == 'active'): ?>
                                        <a class="ops-dd-item" href="?toggle_status=inactive&user_id=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxToggleStatus(this, 'inactive')">🚫 离职</a>
                                        <?php else: ?>
                                        <a class="ops-dd-item" href="?toggle_status=active&user_id=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxToggleStatus(this, 'active')">✅ 启用</a>
                                        <?php endif; ?>
                                        <?php if(!$is_current || $is_super_admin): ?><a class="ops-dd-item ops-dd-danger" href="?delete=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxDeleteUser(this)">删除</a><?php else: ?><span style="color:#999;display:block;padding:8px 10px;">当前</span><?php endif; ?>
                                    </div>
                                </div>
                            </td>
                        </tr>
                        <?php endforeach; ?>
                    </tbody>
                </table>
            </div>
        </div>
        <?php endif; ?>
        
        <!-- 主管区域 -->
        <?php if(!empty($grouped_users['supervisor'])): ?>
        <div class="role-section">
            <div class="role-header"><span class="role-icon">👔</span><span class="role-title">主管</span><span class="role-count"><?php echo count($grouped_users['supervisor']); ?>人</span></div>
            <div style="overflow-x:auto;">
                <table>
                    <thead>
                        <tr>
                            <th class="checkbox-col"><input type="checkbox" class="select-all-supervisor" onclick="toggleSelectByRole('supervisor', this)"></th>
                            <?php if($is_super_admin): ?><th class="col-pw">明文密码</th><?php endif; ?>
                            <th class="col-name">姓名</th>
                            <th class="col-phone">联系电话</th>
                            <th class="col-role">角色</th>
                            <th class="col-sector">赛道</th>
                            <th class="col-profile">权限组</th>
                            <th class="col-status">状态</th>
                            <th class="col-time">注册时间</th>
                            <th class="col-op">操作</th>
                        </tr>
                    </thead>
                    <tbody>
                        <?php foreach($grouped_users['supervisor'] as $u): ?>
                        <tr>
                            <td class="checkbox-col"><input type="checkbox" name="selected_users[]" value="<?php echo $u['id']; ?>" class="user-checkbox" data-role="supervisor"></td>
                            <?php if($is_super_admin): ?>
                            <td class="password-cell"><?php echo htmlspecialchars($u['plain_password'] ?? '-'); ?></td>
                            <?php endif; ?>
                            <td><div style="display:flex;flex-direction:column;align-items:flex-start;gap:1px;"><span style="font-size:10px;color:#888;line-height:1;"><?php echo htmlspecialchars($u['username']); ?></span><div style="display:flex;align-items:center;gap:6px;line-height:1.2;"><?php if(!empty($u['dingtalk_avatar'])): ?><img src="<?php echo htmlspecialchars($u['dingtalk_avatar']); ?>" class="user-avatar" alt="" onerror="this.style.display='none';this.nextElementSibling.style.display='inline'"><?php endif; ?><span class="user-avatar-fallback" style="<?php echo !empty($u['dingtalk_avatar']) ? 'display:none' : ''; ?>">👤</span><span><?php echo htmlspecialchars($u['real_name']); ?><?php $_nb_ts = strtotime((string)($u['created_at'] ?? '')); if(empty($u['skip_newbie']) && $_nb_ts && (time() - $_nb_ts) < 15*86400): ?><span style="font-size:10px;color:#fff;background:#f97316;border-radius:4px;padding:1px 5px;margin-left:5px;">新手</span><?php endif; ?></span></div></div></td>
                            <td style="font-size:13px;color:#555;"><?php echo htmlspecialchars($u['phone'] ?? ''); ?></td>
                            <td><span class="role-badge role-supervisor">主管</span></td>
                            <td class="sector-cell" data-id="<?php echo (int)$u['id']; ?>" data-sector="<?php echo $u['sector'] ? htmlspecialchars($u['sector']) : ''; ?>"><?php echo render_sector_display($u['sector']); ?></td>
                            <td><?php render_profile_select($u, $permission_profiles, $user, $is_super_admin, $selected_company_id); ?></td>
                            <td><span class="status-badge status-<?php echo $u['status']; ?>"><?php echo $u['status'] == 'active' ? '✅ 在职' : '🚫 离职'; ?></span></td>
                            <td><?php echo substr($u['created_at'],0,10); ?></td>
                            <td class="table-actions">
                                <div class="ops-dd">
                                    <button type="button" class="ops-dd-btn" onclick="toggleOpsMenu(event, this)">操作 <span class="ops-dd-caret">▾</span></button>
                                    <div class="ops-dd-menu">
                                        <a class="ops-dd-item" href="?edit=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>">编辑</a>
                                        <?php if($u['status'] == 'active'): ?>
                                        <a class="ops-dd-item" href="?toggle_status=inactive&user_id=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxToggleStatus(this, 'inactive')">🚫 离职</a>
                                        <?php else: ?>
                                        <a class="ops-dd-item" href="?toggle_status=active&user_id=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxToggleStatus(this, 'active')">✅ 启用</a>
                                        <?php endif; ?>
                                        <a class="ops-dd-item ops-dd-danger" href="?delete=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxDeleteUser(this)">删除</a>
                                    </div>
                                </div>
                            </td>
                        </tr>
                        <?php endforeach; ?>
                    </tbody>
                </table>
            </div>
        </div>
        <?php endif; ?>

        <!-- 人事区域 -->
        <?php if(!empty($grouped_users['hr'])): ?>
        <div class="role-section">
            <div class="role-header"><span class="role-icon">📋</span><span class="role-title">人事</span><span class="role-count"><?php echo count($grouped_users['hr']); ?>人</span></div>
            <div style="overflow-x:auto;">
                <table>
                    <thead>
                        <tr>
                            <th class="checkbox-col"><input type="checkbox" class="select-all-hr" onclick="toggleSelectByRole('hr', this)"></th>
                            <?php if($is_super_admin): ?><th class="col-pw">明文密码</th><?php endif; ?>
                            <th class="col-name">姓名</th>
                            <th class="col-phone">联系电话</th>
                            <th class="col-role">角色</th>
                            <th class="col-sector">赛道</th>
                            <th class="col-profile">权限组</th>
                            <th class="col-status">状态</th>
                            <th class="col-time">注册时间</th>
                            <th class="col-op">操作</th>
                        </tr>
                    </thead>
                    <tbody>
                        <?php foreach($grouped_users['hr'] as $u): $is_current=($u['id']==$user['id']); ?>
                        <tr>
                            <td class="checkbox-col"><input type="checkbox" name="selected_users[]" value="<?php echo $u['id']; ?>" class="user-checkbox" data-role="hr"></td>
                            <?php if($is_super_admin): ?>
                            <td class="password-cell"><?php echo htmlspecialchars($u['plain_password'] ?? '-'); ?></td>
                            <?php endif; ?>
                            <td><div style="display:flex;flex-direction:column;align-items:flex-start;gap:1px;"><span style="font-size:10px;color:#888;line-height:1;"><?php echo htmlspecialchars($u['username']); ?></span><div style="display:flex;align-items:center;gap:6px;line-height:1.2;"><?php if(!empty($u['dingtalk_avatar'])): ?><img src="<?php echo htmlspecialchars($u['dingtalk_avatar']); ?>" class="user-avatar" alt="" onerror="this.style.display='none';this.nextElementSibling.style.display='inline'"><?php endif; ?><span class="user-avatar-fallback" style="<?php echo !empty($u['dingtalk_avatar']) ? 'display:none' : ''; ?>">👤</span><span><?php echo htmlspecialchars($u['real_name']); ?><?php $_nb_ts = strtotime((string)($u['created_at'] ?? '')); if(empty($u['skip_newbie']) && $_nb_ts && (time() - $_nb_ts) < 15*86400): ?><span style="font-size:10px;color:#fff;background:#f97316;border-radius:4px;padding:1px 5px;margin-left:5px;">新手</span><?php endif; ?><?php if($is_current): ?><span style="font-size:10px;color:#667eea;margin-left:5px;">(当前)</span><?php endif; ?></span></div></div></td>
                            <td style="font-size:13px;color:#555;"><?php echo htmlspecialchars($u['phone'] ?? ''); ?></td>
                            <td><span class="role-badge role-hr">人事</span></td>
                            <td class="sector-cell" data-id="<?php echo (int)$u['id']; ?>" data-sector="<?php echo $u['sector'] ? htmlspecialchars($u['sector']) : ''; ?>"><?php echo render_sector_display($u['sector']); ?></td>
                            <td><?php render_profile_select($u, $permission_profiles, $user, $is_super_admin, $selected_company_id); ?></td>
                            <td><span class="status-badge status-<?php echo $u['status']; ?>"><?php echo $u['status'] == 'active' ? '✅ 在职' : '🚫 离职'; ?></span></td>
                            <td><?php echo substr($u['created_at'],0,10); ?></td>
                            <td class="action-btns">
                                <div class="ops-dd">
                                    <button type="button" class="ops-dd-btn" onclick="toggleOpsMenu(event, this)">操作 <span class="ops-dd-caret">▾</span></button>
                                    <div class="ops-dd-menu">
                                        <a class="ops-dd-item" href="?edit=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>">编辑</a>
                                        <?php if(!$is_current): ?>
                                        <a class="ops-dd-item" href="?toggle_status=<?php echo $u['status']=='active'?'inactive':'active'; ?>&user_id=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxToggleStatus(this, '<?php echo $u['status']=='active'?'inactive':'active'; ?>')"><?php echo $u['status']=='active'?'🚫 离职':'✅ 启用'; ?></a>
                                        <?php endif; ?>
                                        <?php if(!$is_current || $is_super_admin): ?><a class="ops-dd-item ops-dd-danger" href="?delete=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxDeleteUser(this)">删除</a><?php endif; ?>
                                    </div>
                                </div>
                            </td>
                        </tr>
                        <?php endforeach; ?>
                    </tbody>
                </table>
            </div>
        </div>
        <?php endif; ?>

        <!-- 小组团队区域 -->
        <?php 
        $groups_with_members = [];
        foreach($grouped_users['leader'] as $leader) {
            if($leader['status'] == 'active') {
                $gid = $leader['group_id'] ?: 0;
                if(!isset($groups_with_members[$gid])) $groups_with_members[$gid] = ['leaders' => [], 'members' => []];
                $groups_with_members[$gid]['leaders'][] = $leader;
            }
        }
        foreach($grouped_users['member'] as $member) {
            if($member['status'] == 'active') {
                $gid = $member['group_id'] ?: 0;
                if(!isset($groups_with_members[$gid])) $groups_with_members[$gid] = ['leaders' => [], 'members' => []];
                $groups_with_members[$gid]['members'][] = $member;
            }
        }
        ksort($groups_with_members);
        ?>
        
        <?php if(!empty($groups_with_members)): ?>
        <div class="role-section">
            <div class="role-header"><span class="role-icon">👥</span><span class="role-title">小组团队</span><span class="role-count"><?php echo count($groups_with_members); ?>个小组</span></div>
            
            <?php foreach($groups_with_members as $gid => $group): ?>
            <div style="margin-bottom:20px; background:#f8f9fa; border-radius:12px; overflow:hidden; border:1px solid #e0e0e0;">
                <div style="background:#e8eaf6; padding:10px 15px; font-size:14px; font-weight:bold; color:#3949ab;">
                    📁 小组 <?php echo $gid ?: '未分组'; ?>
                    <span style="font-size:12px; font-weight:normal; margin-left:10px;">
                        共 <?php echo count($group['leaders']) + count($group['members']); ?>人 
                        | 组长：<?php echo !empty($group['leaders']) ? implode(', ', array_column($group['leaders'], 'real_name')) : '无'; ?>
                    </span>
                    <label style="margin-left:15px; font-size:12px; font-weight:normal;">
                        <input type="checkbox" class="select-all-group" data-group="<?php echo $gid; ?>" onclick="toggleSelectByGroup(this)"> 全选本组
                    </label>
                </div>
                <div style="overflow-x:auto;">
                    <table>
                        <thead>
                            <tr>
                                <th class="checkbox-col"><input type="checkbox" class="group-header-checkbox" data-group="<?php echo $gid; ?>" onclick="toggleSelectByGroup(this)"></th>
                                <?php if($is_super_admin): ?><th class="col-pw">明文密码</th><?php endif; ?>
                                <th class="col-name">姓名</th>
                                <th class="col-phone">联系电话</th>
                                <th class="col-role">角色</th>
                                <th class="col-sector">赛道</th>
                                <th class="col-profile">权限组</th>
                                <th class="col-status">状态</th>
                                <th class="col-time">注册时间</th>
                                <th class="col-op">操作</th>
                            </tr>
                        </thead>
                        <tbody>
                            <?php foreach($group['leaders'] as $u): ?>
                            <tr>
                                <td class="checkbox-col"><input type="checkbox" name="selected_users[]" value="<?php echo $u['id']; ?>" class="user-checkbox" data-group="<?php echo $gid; ?>"></td>
                                <?php if($is_super_admin): ?>
                                <td class="password-cell"><?php echo htmlspecialchars($u['plain_password'] ?? '-'); ?></td>
                                <?php endif; ?>
                                <td><div style="display:flex;flex-direction:column;align-items:flex-start;gap:1px;"><span style="font-size:10px;color:#888;line-height:1;"><?php echo htmlspecialchars($u['username']); ?></span><div style="display:flex;align-items:center;gap:6px;line-height:1.2;"><?php if(!empty($u['dingtalk_avatar'])): ?><img src="<?php echo htmlspecialchars($u['dingtalk_avatar']); ?>" class="user-avatar" alt="" onerror="this.style.display='none';this.nextElementSibling.style.display='inline'"><?php endif; ?><span class="user-avatar-fallback" style="<?php echo !empty($u['dingtalk_avatar']) ? 'display:none' : ''; ?>">👤</span><span><?php echo htmlspecialchars($u['real_name']); ?><?php $_nb_ts = strtotime((string)($u['created_at'] ?? '')); if(empty($u['skip_newbie']) && $_nb_ts && (time() - $_nb_ts) < 15*86400): ?><span style="font-size:10px;color:#fff;background:#f97316;border-radius:4px;padding:1px 5px;margin-left:5px;">新手</span><?php endif; ?> <span style="color:#388e3c; font-weight:500;">(组长)</span></span></div></div></td>
                                <td style="font-size:13px;color:#555;"><?php echo htmlspecialchars($u['phone'] ?? ''); ?></td>
                                <td><span class="role-badge role-leader">组长</span></td>
                                <td class="sector-cell" data-id="<?php echo (int)$u['id']; ?>" data-sector="<?php echo $u['sector'] ? htmlspecialchars($u['sector']) : ''; ?>"><?php echo render_sector_display($u['sector']); ?></td>
                                <td><?php render_profile_select($u, $permission_profiles, $user, $is_super_admin, $selected_company_id); ?></td>
                                <td><span class="status-badge status-<?php echo $u['status']; ?>"><?php echo $u['status'] == 'active' ? '✅ 在职' : '🚫 离职'; ?></span></td>
                                <td><?php echo substr($u['created_at'],0,10); ?></td>
                                <td class="table-actions">
                                    <div class="ops-dd">
                                        <button type="button" class="ops-dd-btn" onclick="toggleOpsMenu(event, this)">操作 <span class="ops-dd-caret">▾</span></button>
                                        <div class="ops-dd-menu">
                                            <a class="ops-dd-item" href="?edit=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>">编辑</a>
                                            <a class="ops-dd-item" href="?toggle_status=inactive&user_id=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxToggleStatus(this, 'inactive')">🚫 离职</a>
                                            <a class="ops-dd-item ops-dd-danger" href="?delete=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxDeleteUser(this)">删除</a>
                                        </div>
                                    </div>
                                </td>
                            </tr>
                            <?php endforeach; ?>
                            <?php foreach($group['members'] as $u): ?>
                            <tr>
                                <td class="checkbox-col"><input type="checkbox" name="selected_users[]" value="<?php echo $u['id']; ?>" class="user-checkbox" data-group="<?php echo $gid; ?>"></td>
                                <?php if($is_super_admin): ?>
                                <td class="password-cell"><?php echo htmlspecialchars($u['plain_password'] ?? '-'); ?></td>
                                <?php endif; ?>
                                <td><div style="display:flex;flex-direction:column;align-items:flex-start;gap:1px;"><span style="font-size:10px;color:#888;line-height:1;"><?php echo htmlspecialchars($u['username']); ?></span><div style="display:flex;align-items:center;gap:6px;line-height:1.2;"><?php if(!empty($u['dingtalk_avatar'])): ?><img src="<?php echo htmlspecialchars($u['dingtalk_avatar']); ?>" class="user-avatar" alt="" onerror="this.style.display='none';this.nextElementSibling.style.display='inline'"><?php endif; ?><span class="user-avatar-fallback" style="<?php echo !empty($u['dingtalk_avatar']) ? 'display:none' : ''; ?>">👤</span><span><?php echo htmlspecialchars($u['real_name']); ?><?php $_nb_ts = strtotime((string)($u['created_at'] ?? '')); if(empty($u['skip_newbie']) && $_nb_ts && (time() - $_nb_ts) < 15*86400): ?><span style="font-size:10px;color:#fff;background:#f97316;border-radius:4px;padding:1px 5px;margin-left:5px;">新手</span><?php endif; ?></span></div></div></td>
                                <td style="font-size:13px;color:#555;"><?php echo htmlspecialchars($u['phone'] ?? ''); ?></td>
                                <td><span class="role-badge role-member">组员</span></td>
                                <td class="sector-cell" data-id="<?php echo (int)$u['id']; ?>" data-sector="<?php echo $u['sector'] ? htmlspecialchars($u['sector']) : ''; ?>"><?php echo render_sector_display($u['sector']); ?></td>
                                <td><?php render_profile_select($u, $permission_profiles, $user, $is_super_admin, $selected_company_id); ?></td>
                                <td><span class="status-badge status-<?php echo $u['status']; ?>"><?php echo $u['status'] == 'active' ? '✅ 在职' : '🚫 离职'; ?></span></td>
                                <td><?php echo substr($u['created_at'],0,10); ?></td>
                                <td class="table-actions">
                                    <div class="ops-dd">
                                        <button type="button" class="ops-dd-btn" onclick="toggleOpsMenu(event, this)">操作 <span class="ops-dd-caret">▾</span></button>
                                        <div class="ops-dd-menu">
                                            <a class="ops-dd-item" href="?edit=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>">编辑</a>
                                            <a class="ops-dd-item" href="?toggle_status=inactive&user_id=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxToggleStatus(this, 'inactive')">🚫 离职</a>
                                            <a class="ops-dd-item ops-dd-danger" href="?delete=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxDeleteUser(this)">删除</a>
                                        </div>
                                    </div>
                                </td>
                            </tr>
                            <?php endforeach; ?>
                        </tbody>
                    </table>
                </div>
            </div>
            <?php endforeach; ?>
        </div>
        <?php endif; ?>
        
        <!-- 离职人员区域 -->
        <?php 
        $inactive_users_list = [];
        foreach($users_list as $u) {
            if($u['status'] == 'inactive') {
                $inactive_users_list[] = $u;
            }
        }
        ?>
        <?php if(!empty($inactive_users_list)): ?>
        <div style="margin-top:30px; border-top:2px solid #ffebee; padding-top:20px;">
            <div style="background:#ffebee; padding:10px 15px; border-radius:12px; margin-bottom:15px; display:flex; align-items:center; gap:10px;">
                <span style="font-size:20px;">🚫</span>
                <span style="font-weight:bold;">离职/禁用人员</span>
                <span style="background:#f0f0f0; color:#666; padding:2px 10px; border-radius:20px; font-size:12px;"><?php echo count($inactive_users_list); ?>人</span>
                <label style="margin-left:15px; font-size:12px;">
                    <input type="checkbox" id="select-all-inactive" onclick="toggleSelectAllInactive(this)"> 全选离职人员
                </label>
            </div>
            <div style="overflow-x:auto;">
                <table>
                    <thead>
                        <tr>
                            <th class="checkbox-col"><input type="checkbox" id="inactive-header-checkbox" onclick="toggleSelectAllInactive(this)"></th>
                            <?php if($is_super_admin): ?><th class="col-pw">明文密码</th><?php endif; ?>
                            <th class="col-name">姓名</th>
                            <th class="col-phone">联系电话</th>
                            <th class="col-role">角色</th>
                            <th class="col-sector">赛道</th>
                            <th class="col-profile">权限组</th>
                            <th class="col-status">状态</th>
                            <th class="col-time">注册时间</th>
                            <th class="col-time">离职时间</th>
                            <th class="col-op">操作</th>
                        </tr>
                    </thead>
                    <tbody>
                        <?php foreach($inactive_users_list as $u): ?>
                        <tr style="background:#fff5f5;">
                            <td class="checkbox-col"><input type="checkbox" name="selected_users[]" value="<?php echo $u['id']; ?>" class="user-checkbox inactive-checkbox"></td>
                            <?php if($is_super_admin): ?>
                            <td class="password-cell"><?php echo htmlspecialchars($u['plain_password'] ?? '-'); ?></td>
                            <?php endif; ?>
                            <td><div style="display:flex;flex-direction:column;align-items:flex-start;gap:1px;"><span style="font-size:10px;color:#888;line-height:1;"><?php echo htmlspecialchars($u['username']); ?></span><div style="display:flex;align-items:center;gap:6px;line-height:1.2;"><?php if(!empty($u['dingtalk_avatar'])): ?><img src="<?php echo htmlspecialchars($u['dingtalk_avatar']); ?>" class="user-avatar" alt="" onerror="this.style.display='none';this.nextElementSibling.style.display='inline'"><?php endif; ?><span class="user-avatar-fallback" style="<?php echo !empty($u['dingtalk_avatar']) ? 'display:none' : ''; ?>">👤</span><span><?php echo htmlspecialchars($u['real_name']); ?><?php $_nb_ts = strtotime((string)($u['created_at'] ?? '')); if(empty($u['skip_newbie']) && $_nb_ts && (time() - $_nb_ts) < 15*86400): ?><span style="font-size:10px;color:#fff;background:#f97316;border-radius:4px;padding:1px 5px;margin-left:5px;">新手</span><?php endif; ?></span></div></div></td>
                            <td style="font-size:13px;color:#555;"><?php echo htmlspecialchars($u['phone'] ?? ''); ?></td>
                            <td>
                                <?php
                                switch($u['role']) {
                                    case 'manager': echo '<span class="role-badge role-manager">经理</span>'; break;
                                    case 'hr': echo '<span class="role-badge role-hr">人事</span>'; break;
                                    case 'supervisor': echo '<span class="role-badge role-supervisor">主管</span>'; break;
                                    case 'leader': echo '<span class="role-badge role-leader">组长</span>'; break;
                                    default: echo '<span class="role-badge role-member">组员</span>'; break;
                                }
                                ?>
                            </td>
                            <td class="sector-cell" data-id="<?php echo (int)$u['id']; ?>" data-sector="<?php echo $u['sector'] ? htmlspecialchars($u['sector']) : ''; ?>"><?php echo render_sector_display($u['sector']); ?></td>
                            <td><?php render_profile_select($u, $permission_profiles, $user, $is_super_admin, $selected_company_id); ?></td>
                            <td><span class="status-badge status-inactive">🚫 离职/禁用</span></td>
                            <td><?php echo substr($u['created_at'],0,10); ?></td>
                            <td><?php echo isset($u['resignation_date']) && $u['resignation_date'] ? $u['resignation_date'] : '-'; ?></td>
                            <td class="table-actions">
                                <div class="ops-dd">
                                    <button type="button" class="ops-dd-btn" onclick="toggleOpsMenu(event, this)">操作 <span class="ops-dd-caret">▾</span></button>
                                    <div class="ops-dd-menu">
                                        <a class="ops-dd-item" href="?edit=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>">编辑</a>
                                        <a class="ops-dd-item" href="?toggle_status=active&user_id=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxToggleStatus(this, 'active')">✅ 启用</a>
                                        <a class="ops-dd-item ops-dd-danger" href="?delete=<?php echo $u['id']; ?><?php echo $is_super_admin ? '&company_id=' . $selected_company_id : ''; ?>" onclick="return ajaxDeleteUser(this)">删除</a>
                                    </div>
                                </div>
                            </td>
                        </tr>
                        <?php endforeach; ?>
                    </tbody>
                </table>
            </div>
        </div>
        <?php endif; ?>
        
        <?php if(empty($users_list)): ?>
        <div style="text-align:center; padding:40px; color:#999;">暂无成员，请点击上方添加用户</div>
        <?php endif; ?>
    </div>
</div>

<script>
var USER_MODAL_DATA = <?php echo json_encode($modal_users, JSON_UNESCAPED_UNICODE | JSON_HEX_TAG | JSON_HEX_APOS | JSON_HEX_QUOT | JSON_HEX_AMP); ?>;
var INITIAL_EDIT_ID = <?php echo $edit_user ? (int)$edit_user['id'] : 'null'; ?>;
var CURRENT_USER_ID = <?php echo (int)$user['id']; ?>;
var IS_SUPER_ADMIN = <?php echo $is_super_admin ? 'true' : 'false'; ?>;

function setFieldValue(id, value) {
    var el = document.getElementById(id);
    if (el) el.value = value == null ? '' : value;
}

// ===== 权限组 JS =====
var PERMISSION_PROFILES = <?php echo $profiles_json; ?>;
var DEFAULT_PROFILE_ID = <?php echo (int)$default_profile_id; ?>;

function openProfileModal(id, name, desc, perms, companyId) {
    var mask = document.getElementById('profileModalMask');
    var title = document.getElementById('profileModalTitle');
    if (!mask) return;
    var editing = !!id;
    document.getElementById('profileModalId').value = id || 0;
    document.getElementById('profileModalName').value = name || '';
    document.getElementById('profileModalDesc').value = desc || '';
    document.getElementById('profileModalTitle').textContent = editing ? '🎛️ 编辑权限组' : '🎛️ 新建权限组';
    document.querySelectorAll('.profile-perm-check').forEach(function(cb) {
        var f = cb.getAttribute('data-field');
        cb.checked = perms ? Number(perms[f] || 0) === 1 : false;
    });
    mask.classList.add('show');
}

function closeProfileModal(ev) {
    var mask = document.getElementById('profileModalMask');
    if (ev && ev.target !== mask) return;
    if (mask) mask.classList.remove('show');
}

function setProfilePermChecks(checked) {
    document.querySelectorAll('.profile-perm-check').forEach(function(cb) { cb.checked = !!checked; });
}

function deleteProfile(id, name) {
    if (!confirm('确定删除权限组「' + name + '」吗？此操作不影响已应用该组的员工现有权限。')) return;
    var f = document.createElement('form');
    f.method = 'POST';
    f.action = 'users.php<?php echo $is_super_admin ? '?company_id=' . $selected_company_id : ''; ?>';
    var a = document.createElement('input'); a.type='hidden'; a.name='action'; a.value='profile_delete';
    var i = document.createElement('input'); i.type='hidden'; i.name='profile_id'; i.value=id;
    var c = document.createElement('input'); c.type='hidden'; c.name='company_id'; c.value='<?php echo (int)$selected_company_id; ?>';
    f.appendChild(a); f.appendChild(i); f.appendChild(c);
    document.body.appendChild(f); f.submit();
}

function applyPermissionProfileToModal(profileId) {
    if (!profileId) return;
    var p = null;
    for (var i = 0; i < PERMISSION_PROFILES.length; i++) {
        if (String(PERMISSION_PROFILES[i].id) === String(profileId)) { p = PERMISSION_PROFILES[i]; break; }
    }
    if (!p || !p.perms) { showToast('权限组数据未找到', 'error'); return; }
    var perms = p.perms;
    var lp = document.getElementById('modalLoginPromptEnabled');
    if (lp) lp.checked = Number(perms['login_prompt_enabled'] || 0) === 1;
    var cr = document.getElementById('modalCheckinReminderEnabled');
    if (cr) cr.checked = Number(perms['checkin_reminder_enabled'] || 0) === 1;
    showToast('已套用权限组「' + p.name + '」', 'success');
}

function toggleSectorChip(btn) {
    var input = document.getElementById('modalSector');
    if (!input) return;
    var s = (btn.getAttribute('data-sector') || '').trim();
    var current = input.value.split(',').map(function(v){return v.trim();}).filter(Boolean);
    var idx = current.indexOf(s);
    if (idx >= 0) {
        current.splice(idx, 1);
        btn.style.background = 'white';
        btn.style.color = '#555';
    } else {
        current.push(s);
        btn.style.background = '#667eea';
        btn.style.color = 'white';
        btn.style.borderColor = '#667eea';
    }
    input.value = current.join(',');
}

function delReservedSector(delEl) {
    var chip = delEl.closest('.sector-chip');
    if (!chip) return;
    var name = chip.getAttribute('data-sector');
    if (!confirm('确认删除系统预设「' + name + '」？删除后将不再作为预设显示。')) return;
    var url = 'users.php?rs_action=del&name=' + encodeURIComponent(name);
    fetch(url, { credentials: 'same-origin' })
      .then(function(r){ return r.json(); })
      .then(function(data){
          if (data.success) {
              refreshSectorPresets(data.presets);
              showToast(data.message, 'success');
          } else {
              showToast(data.message || '删除失败', 'error');
          }
      })
      .catch(function(){ showToast('网络错误', 'error'); });
}

function openReservedSectorAdd(btn) {
    var name = prompt('请输入要添加的系统预设赛道名称：');
    if (!name) return;
    name = name.trim();
    if (!name) { showToast('名称不能为空', 'warning'); return; }
    var url = 'users.php?rs_action=add&name=' + encodeURIComponent(name);
    fetch(url, { credentials: 'same-origin' })
      .then(function(r){ return r.json(); })
      .then(function(data){
          if (data.success) {
              refreshSectorPresets(data.presets);
              showToast(data.message, 'success');
          } else {
              showToast(data.message || '添加失败', 'error');
          }
      })
      .catch(function(){ showToast('网络错误', 'error'); });
}

// 用服务端返回的预设列表全量刷新赛道预设按钮组
function refreshSectorPresets(presets) {
    RESERVED_SECTORS = presets || [];
    var container = document.getElementById('sectorReservedOptions');
    if (container) container.innerHTML = '';
    if (container && presets) {
        presets.forEach(function(name){
            var newChip = document.createElement('button');
            newChip.type = 'button';
            newChip.className = 'sector-chip';
            newChip.setAttribute('data-sector', name);
            newChip.setAttribute('onclick', 'toggleSectorChip(this)');
            newChip.style.cssText = 'padding:4px 10px;border:1px solid #ddd;border-radius:20px;background:white;color:#555;font-size:12px;cursor:pointer;';
            newChip.innerHTML = name + '<span class="rs-del" onclick="event.stopPropagation();delReservedSector(this)" style="margin-left:5px;color:#bbb;cursor:pointer;">✕</span>';
            container.appendChild(newChip);
        });
        var addBtn = document.createElement('button');
        addBtn.type = 'button';
        addBtn.setAttribute('onclick', 'openReservedSectorAdd(this)');
        addBtn.style.cssText = 'padding:4px 10px;border:1px dashed #bbb;border-radius:20px;background:#fff;color:#999;font-size:12px;cursor:pointer;';
        addBtn.textContent = '＋ 添加系统预设';
        container.appendChild(addBtn);
    }
}

function openUserModal(mode, userId) {
    var mask = document.getElementById('userModalMask');
    var title = document.getElementById('userModalTitle');
    var action = document.getElementById('modalAction');
    var submitBtn = document.getElementById('modalSubmitBtn');
    var username = document.getElementById('modalUsername');
    var password = document.getElementById('modalPassword');
    var passwordLabel = document.getElementById('modalPasswordLabel');
    var modalProfileSel = document.getElementById('modalPermissionProfile');
    if (modalProfileSel) modalProfileSel.value = DEFAULT_PROFILE_ID ? String(DEFAULT_PROFILE_ID) : '';
    // 添加/编辑用户：权限组均可自由改选（添加时默认选中「组员」）
    if (modalProfileSel) {
        modalProfileSel.disabled = false;
        modalProfileSel.classList.remove('profile-locked');
        modalProfileSel.onchange = function(){ applyPermissionProfileToModal(this.value); };
    }
    if (!mask) return;

    if (mode === 'edit') {
        var data = USER_MODAL_DATA[String(userId)];
        if (!data) {
            showToast('未找到用户数据，请刷新页面后重试', 'error');
            return;
        }
        title.textContent = '✏️ 编辑用户';
        action.value = 'edit';
        submitBtn.textContent = '保存修改';
        setFieldValue('modalUserId', data.id);
        setFieldValue('modalUsername', data.username);
        setFieldValue('modalRealName', data.real_name);
        setFieldValue('modalPhone', data.phone || '');
        setFieldValue('modalRole', data.role || 'member');
        setFieldValue('modalGroupId', data.group_id || '');
        setFieldValue('modalStatus', data.status || 'active');
        setFieldValue('modalResignationDate', data.resignation_date || '');
        setFieldValue('modalSector', data.sector || '');
        setFieldValue('modalSkipNewbie', data.skip_newbie ? true : false);
        password.value = '';
        password.placeholder = '留空则不修改密码';
        passwordLabel.textContent = '新密码(留空则不修改)';
        username.readOnly = false;
        username.style.background = '';
        resetUsernameStatus();
        setFieldValue('modalPermissionProfile', data.permission_profile_id ? String(data.permission_profile_id) : '');
        setFieldValue('modalManagedGroupIds', data.managed_group_ids || '');
        var loginPromptEnabled = document.getElementById('modalLoginPromptEnabled');
        if (loginPromptEnabled) loginPromptEnabled.checked = Number(data.login_prompt_enabled || 0) === 1;
        setFieldValue('modalLoginPromptText', data.login_prompt_text || '');
        var checkinReminderEnabled = document.getElementById('modalCheckinReminderEnabled');
        if (checkinReminderEnabled) checkinReminderEnabled.checked = Number(data.checkin_reminder_enabled || 0) === 1;
    } else {
        title.textContent = '➕ 添加新用户';
        action.value = 'add';
        submitBtn.textContent = '添加用户';
        setFieldValue('modalUserId', '');
        setFieldValue('modalUsername', '');
        setFieldValue('modalRealName', '');
        setFieldValue('modalPhone', '');
        setFieldValue('modalRole', 'member');
        setFieldValue('modalGroupId', '');
        setFieldValue('modalStatus', 'active');
        setFieldValue('modalResignationDate', '');
        password.value = '123456';
        password.placeholder = '默认123456';
        passwordLabel.textContent = '密码';
        username.readOnly = false;
        username.style.background = '';
        resetUsernameStatus();
        // 添加新用户时，若有「组员/员工」默认权限组则自动套用
        if (DEFAULT_PROFILE_ID) applyPermissionProfileToModal(DEFAULT_PROFILE_ID);
        setFieldValue('modalManagedGroupIds', '');
        setFieldValue('modalSector', '');
        setFieldValue('modalSkipNewbie', false);
        var loginPromptEnabledAdd = document.getElementById('modalLoginPromptEnabled');
        if (loginPromptEnabledAdd) loginPromptEnabledAdd.checked = false;
        setFieldValue('modalLoginPromptText', '');
        var checkinReminderAdd = document.getElementById('modalCheckinReminderEnabled');
        if (checkinReminderAdd) checkinReminderAdd.checked = false;
    }
    mask.classList.add('show');
    setTimeout(function() {
        (mode === 'edit' ? document.getElementById('modalRealName') : username).focus();
    }, 30);
}

// 用户名实时查重与提示
function resetUsernameStatus() {
    var hint = document.getElementById('usernameStatus');
    if (hint) { hint.style.display = 'none'; hint.dataset.exists = '0'; }
}

function checkUsernameDup() {
    var input = document.getElementById('modalUsername');
    var hint = document.getElementById('usernameStatus');
    var userIdInput = document.getElementById('modalUserId');
    if (!input || !hint) return;
    var v = input.value.trim();
    if (!v) { resetUsernameStatus(); return; }
    var userId = (userIdInput && userIdInput.value) ? userIdInput.value : 0;
    fetch('users.php?check_username=' + encodeURIComponent(v) + '&user_id=' + encodeURIComponent(userId), { credentials: 'same-origin' })
    .then(function(r) { return r.json(); })
    .then(function(data) {
        if (data && data.exists) {
            hint.style.display = 'block';
            hint.dataset.exists = '1';
        } else {
            hint.style.display = 'none';
            hint.dataset.exists = '0';
        }
    })
    .catch(function() {});
}

(function() {
    var form = document.getElementById('userEditForm');
    if (form) {
        form.addEventListener('submit', function(e) {
            var hint = document.getElementById('usernameStatus');
            if (hint && hint.dataset.exists === '1') {
                e.preventDefault();
                showToast('用户名已存在，请更换后再保存', 'error');
                document.getElementById('modalUsername').focus();
            }
        });
    }
})();

function toggleOpsMenu(ev, btn) {
    if (ev) { ev.preventDefault(); ev.stopPropagation(); }
    if (!btn) return;
    var dd = btn.closest('.ops-dd');
    if (!dd) return;
    var wasOpen = dd.classList.contains('open');
    document.querySelectorAll('.ops-dd.open').forEach(function(d) { d.classList.remove('open'); });
    if (!wasOpen) dd.classList.add('open');
}
document.addEventListener('click', function(e) {
    document.querySelectorAll('.ops-dd.open').forEach(function(d) {
        if (!d.contains(e.target)) d.classList.remove('open');
    });
});

function closeUserModal(event) {
    if (event && event.target !== document.getElementById('userModalMask')) return;
    var mask = document.getElementById('userModalMask');
    if (mask) mask.classList.remove('show');
}

document.querySelectorAll('a.btn-edit').forEach(function(link) {
    link.addEventListener('click', function(e) {
        var match = (link.getAttribute('href') || '').match(/[?&]edit=(\d+)/);
        if (!match) return;
        e.preventDefault();
        openUserModal('edit', match[1]);
    });
});

document.addEventListener('keydown', function(e) {
    if (e.key === 'Escape') closeUserModal();
});

if (INITIAL_EDIT_ID) {
    openUserModal('edit', INITIAL_EDIT_ID);
}

function toggleSelectByGroup(element) {
    var groupId = element.getAttribute('data-group');
    var checkboxes = document.querySelectorAll('.user-checkbox[data-group="'+groupId+'"]');
    checkboxes.forEach(function(cb) { cb.checked = element.checked; });
}

function toggleSelectAllInactive(element) {
    var checkboxes = document.querySelectorAll('.inactive-checkbox');
    checkboxes.forEach(function(cb) { cb.checked = element.checked; });
}

function toggleSelectByRole(role, element) {
    var checkboxes = document.querySelectorAll('.user-checkbox[data-role="'+role+'"]');
    checkboxes.forEach(function(cb) { cb.checked = element.checked; });
}

function togglePermFold(id) {
    var fold = document.getElementById('permfold-' + id);
    if (!fold) return;
    fold.classList.toggle('expanded');
    var t = fold.querySelector('.perm-fold-toggle .pf-caret');
    if (t) t.style.transform = fold.classList.contains('expanded') ? 'rotate(180deg)' : '';
}

function applyUserProfile(sel, userId, profileId) {
    var base = 'users.php<?php echo $is_super_admin ? '?company_id=' . $selected_company_id : ''; ?>';
    var fd = new FormData();
    if (!profileId) {
        fd.append('action', 'profile_unlink');
    } else {
        fd.append('action', 'profile_apply_to_user');
        fd.append('profile_id', profileId);
    }
    fd.append('target_user_id', userId);
    fetch(base, { method: 'POST', headers: { 'X-Requested-With': 'XMLHttpRequest' }, body: fd })
        .then(function(r){ return r.json(); })
        .then(function(res){
            if (res.ok) { showToast(res.msg, 'success'); }
            else { showToast(res.msg, 'error'); }
        })
        .catch(function(){ showToast('操作失败，请重试', 'error'); });
}

function showToast(msg, type) {
    var toast = document.createElement('div');
    toast.className = 'toast toast-' + (type || 'success');
    toast.textContent = msg;
    document.body.appendChild(toast);
    setTimeout(function() { toast.classList.add('toast-show'); }, 10);
    setTimeout(function() {
        toast.classList.remove('toast-show');
        setTimeout(function() { toast.remove(); }, 300);
    }, 2000);
}

function togglePermBadge(el, userId, permKey, newValue, label, companyId) {
    var oldOn = el.classList.contains('perm-on');
    var newVal = oldOn ? 0 : 1; var newClass = newVal == 1 ? 'perm-on' : 'perm-off';
    var oldText = el.textContent;

    // 乐观更新
    el.classList.remove('perm-on', 'perm-off');
    el.classList.add(newClass);
    el.textContent = label + (newVal == 1 ? '开' : '关');
    el.style.pointerEvents = 'none';
    el.style.opacity = '0.6';

    var form = new FormData();
    form.append('action', 'toggle_permission_ajax');
    form.append('user_id', userId);
    form.append('permission', permKey);
    form.append('value', newVal);
    if (companyId && companyId > 0) {
        form.append('company_id', companyId);
    }

    fetch('users.php', {
        method: 'POST',
        body: form,
        credentials: 'same-origin'
    })
    .then(function(r) { return r.json(); })
    .then(function(data) {
        el.style.pointerEvents = '';
        el.style.opacity = '';
        if (data.success) {
            showToast(data.message, 'success');
        } else {
            el.classList.remove('perm-on', 'perm-off');
            el.classList.add(oldOn ? 'perm-on' : 'perm-off');
            el.textContent = oldText;
            showToast(data.message || '操作失败', 'error');
        }
    })
    .catch(function(e) {
        el.style.pointerEvents = '';
        el.style.opacity = '';
        el.classList.remove('perm-on', 'perm-off');
        el.classList.add(oldOn ? 'perm-on' : 'perm-off');
        el.textContent = oldText;
        showToast('网络错误', 'error');
    });
}

function ajaxToggleStatus(link, newStatus) {
    var href = link.getAttribute('href');
    var url = href + (href.indexOf('?') >= 0 ? '&' : '?') + 'ajax=1';

    link.style.pointerEvents = 'none';
    link.style.opacity = '0.6';

    fetch(url, { credentials: 'same-origin' })
    .then(function(r) { return r.json(); })
    .then(function(data) {
        link.style.pointerEvents = '';
        link.style.opacity = '';
        if (data.success) {
            showToast(data.message, 'success');
            setTimeout(function() { location.reload(); }, 800);
        } else {
            showToast(data.message || '操作失败', 'error');
        }
    })
    .catch(function(e) {
        link.style.pointerEvents = '';
        link.style.opacity = '';
        showToast('网络错误', 'error');
    });
    return false;
}

function ajaxDeleteUser(link) {
    var href = link.getAttribute('href');
    var url = href + (href.indexOf('?') >= 0 ? '&' : '?') + 'ajax=1';

    link.style.pointerEvents = 'none';
    link.style.opacity = '0.6';

    fetch(url, { credentials: 'same-origin' })
    .then(function(r) { return r.json(); })
    .then(function(data) {
        link.style.pointerEvents = '';
        link.style.opacity = '';
        if (data.success) {
            showToast(data.message, 'success');
            setTimeout(function() { location.reload(); }, 800);
        } else {
            showToast(data.message || '操作失败', 'error');
        }
    })
    .catch(function(e) {
        link.style.pointerEvents = '';
        link.style.opacity = '';
        showToast('网络错误', 'error');
    });
    return false;
}

// 页面加载时显示 PHP 侧的消息为 toast
document.addEventListener('DOMContentLoaded', function() {
    var msgDiv = document.getElementById('pageMessage');
    if (msgDiv) {
        showToast(msgDiv.dataset.msg, msgDiv.dataset.type);
    }
});
</script>

<script>
// ===== 赛道单元格快速编辑 =====
var SECTOR_PRESETS = <?php echo json_encode($system_sector_presets, JSON_UNESCAPED_UNICODE); ?>;
var RESERVED_SECTORS = SECTOR_PRESETS;
function renderSectorHtml(s){
    if (!s) return '-';
    function escS(x){ return String(x==null?'':x).replace(/[&<>"']/g,function(c){return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c];}); }
    var parts = String(s).split(',').map(function(x){return x.trim();}).filter(Boolean);
    var html = [];
    parts.forEach(function(p){
        if (p) html.push('<span class="sector-item">' + escS(p) + '</span>');
    });
    return html.join('');
}
(function(){
    var style = document.createElement('style');
    style.textContent = [
        'td.sector-cell{cursor:pointer;position:relative;}',
        'td.sector-cell:hover{background:#f5f7ff;}',
        '.sector-editor-wrap{position:absolute;left:0;top:100%;z-index:120;min-width:220px;background:#fff;border:1px solid #ddd;border-radius:10px;box-shadow:0 6px 20px rgba(0,0,0,.15);padding:10px;font-size:13px;color:#333;text-align:left;}',
        '.sector-editor-wrap .se-head{font-weight:600;margin-bottom:8px;}',
        '.sector-editor-wrap .se-chips{display:flex;gap:6px;flex-wrap:wrap;margin-bottom:8px;}',
        '.sector-editor-wrap .se-chip{padding:4px 10px;border:1px solid #ddd;border-radius:20px;background:#fff;color:#555;font-size:12px;cursor:pointer;}',
        '.sector-editor-wrap .se-chip:hover{border-color:#667eea;color:#667eea;}',
        '.sector-editor-wrap .se-chip.on{background:#667eea;color:#fff;border-color:#667eea;}',
        '.sector-editor-wrap .se-free{width:100%;box-sizing:border-box;padding:6px 8px;border:1px solid #ddd;border-radius:6px;font-size:12px;margin-bottom:8px;}',
        '.sector-editor-wrap .se-btns{display:flex;gap:8px;justify-content:flex-end;align-items:center;}',
        '.sector-editor-wrap .se-btns button{cursor:pointer;}',
        '.sector-editor-wrap .se-cancel{background:#fff;border:1px solid #ddd;border-radius:6px;padding:4px 12px;font-size:12px;color:#555;}'
    ].join('');
    document.head.appendChild(style);

    function esc(s){ return String(s==null?'':s).replace(/[&<>"']/g,function(c){return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c];}); }

    function removeEditor(td){
        var w=td.querySelector('.sector-editor-wrap');
        if(w){ w.remove(); }
        td.classList.remove('sector-editing');
        // 若期间保存过新值，关闭时以最新已保存值渲染单元格内容
        if (td._sectorSaved !== undefined) {
            td.setAttribute('data-sector', td._sectorSaved);
            td.innerHTML = renderSectorHtml(td._sectorSaved);
        }
    }

    function closeAllEditors(){ document.querySelectorAll('td.sector-cell.sector-editing').forEach(function(td){ removeEditor(td); }); }

    function openEditor(td){
        if (td.querySelector('.sector-editor-wrap')) return;
        closeAllEditors();
        var current = String(td.getAttribute('data-sector')||'').split(',').map(function(s){return s.trim();}).filter(Boolean);
        var wrap = document.createElement('div');
        wrap.className = 'sector-editor-wrap';
        var head = document.createElement('div'); head.className='se-head'; head.textContent='赛道（可多选）';
        var chipsRow = document.createElement('div'); chipsRow.className='se-chips';
        var free = document.createElement('input'); free.type='text'; free.className='se-free'; free.placeholder='自定义赛道，用逗号分隔';
        var btns = document.createElement('div'); btns.className='se-btns';
        var save = document.createElement('button'); save.type='button'; save.className='btn-status btn-status-active se-save'; save.textContent='完成';
        btns.appendChild(save);

        // 系统预设分组
        var preHead = document.createElement('div'); preHead.className='se-head'; preHead.textContent='系统预设';
        chipsRow.appendChild(preHead);
        SECTOR_PRESETS.forEach(function(s){
            var b = document.createElement('button');
            b.type='button'; b.className='se-chip'+(current.indexOf(s)>=0?' on':''); b.textContent=s;
            b.onclick = function(){ b.classList.toggle('on'); doSave(); };
            chipsRow.appendChild(b);
        });
        free.value = '';
        var savedVal = current.join(',');

        // 点击赛道实时保存（增删），保持弹窗打开；free 作为可选的自定义追加项
        var saveSeq = 0;
        function doSave(){
            // 选中chips + free中不在选中里的自定义赛道
            var selected=[];
            chipsRow.querySelectorAll('.se-chip.on').forEach(function(c){ selected.push(c.textContent.trim()); });
            var freeAdd=[];
            free.value.split(',').forEach(function(x){ x=x.trim(); if(x && selected.indexOf(x)<0) freeAdd.push(x); });
            var val = selected.concat(freeAdd).join(',');
            if (val === savedVal) return; // 无变化不请求
            var mySeq = ++saveSeq;
            var url = '?quick_sector=1&user_id=' + encodeURIComponent(td.getAttribute('data-id'))
                    + '&sector=' + encodeURIComponent(val) + '&ajax=1';
            save.disabled = true; save.textContent = '保存中...';
            fetch(url, { credentials:'same-origin' })
              .then(function(r){ return r.json(); })
              .then(function(data){
                  if (data.success && mySeq === saveSeq){ // 仅采用最后一次请求结果，避免乱序
                      savedVal = data.sector || '';
                      // 暂存已保存的最新值，不在编辑进行中直接重写单元格内容（否则会销毁编辑器）
                      td._sectorSaved = savedVal;
                      td.setAttribute('data-sector', savedVal);
                      // 同步已选 chip 的高亮状态，让界面持续正确
                      var sel = savedVal.split(',').map(function(x){return x.trim();}).filter(Boolean);
                      chipsRow.querySelectorAll('.se-chip').forEach(function(c){
                          c.classList.toggle('on', sel.indexOf(c.textContent.trim()) >= 0);
                      });
                      save.disabled = false; save.textContent = '完成';
                      showToast(data.message || '已更新', 'success');
                  } else if (data.success) {
                      // 已被更新的旧请求，忽略，不打断用户
                      save.disabled = false; save.textContent = '完成';
                  } else {
                      save.disabled = false; save.textContent = '完成';
                      showToast(data.message || '更新失败', 'error');
                  }
              })
              .catch(function(){ if(mySeq===saveSeq){ save.disabled=false; save.textContent='完成'; showToast('网络错误','error'); } });
        }

        // 保存(完成)按钮：仅关闭弹窗，数据已实时保存
        save.onclick = function(){ removeEditor(td); };

        wrap.appendChild(head); wrap.appendChild(chipsRow); wrap.appendChild(free); wrap.appendChild(btns);
        td.appendChild(wrap); td.classList.add('sector-editing');

        setTimeout(function(){
            document.addEventListener('click', function onDoc(e){
                if (e.target.closest('.sector-editor-wrap')) return;
                closeAllEditors();
                document.removeEventListener('click', onDoc);
            });
        }, 0);
    }

    document.querySelectorAll('td.sector-cell').forEach(function(td){
        td.title = '点击修改赛道';
        td.addEventListener('click', function(e){ e.stopPropagation(); openEditor(td); });
    });
})();
</script>
</body>
</html>