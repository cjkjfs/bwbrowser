<?php
/**
 * 快捷复制面板 - 管理员专用
 * 功能：切换公司、显示账号、复制、TOTP验证码自动刷新、手机验证码
 * 权限：经理、超级管理员
 */

require_once 'config.php';
require_once 'company_config.php';
check_auth();

// ========== 日志路径定义 ==========
define('LOG_DIR', __DIR__ . '/logs/');
if (!is_dir(LOG_DIR)) {
    mkdir(LOG_DIR, 0755, true);
}

// IP检测日志文件
define('IP_LOG_FILE', LOG_DIR . 'ip_detect.log');

// 记录IP检测日志的函数
function logIPDetect($message, $data = null) {
    $log = '[' . date('Y-m-d H:i:s') . '] ' . $message;
    if ($data !== null) {
        $log .= ' - ' . print_r($data, true);
    }
    $log .= PHP_EOL;
    file_put_contents(IP_LOG_FILE, $log, FILE_APPEND);
}

$user = get_logged_user($pdo);
$company_id = get_current_company_id($pdo);
$is_super_admin = is_super_admin($pdo);

// 权限检查：只有经理或超级管理员可以访问
if (!$is_super_admin && $user['role'] != 'manager') {
    header('Location: dashboard.php');
    exit;
}

// 允许跨域访问
header('Content-Type: text/html; charset=utf-8');
header("Access-Control-Allow-Origin: *");

// ========== AJAX：更新手机编号 device_id ==========
if (($_SERVER['REQUEST_METHOD'] === 'POST') && isset($_POST['action']) && $_POST['action'] === 'update_device') {
    header('Content-Type: application/json; charset=utf-8');

    $account_id = isset($_POST['id']) ? intval($_POST['id']) : 0;
    $device_id = isset($_POST['device_id']) ? trim($_POST['device_id']) : '';

    if ($account_id <= 0) {
        echo json_encode(['ok' => false, 'msg' => '账号ID不正确']);
        exit;
    }

    // 校验归属：超管可改；管理员只能改本公司账号
    try {
        $stmt = $pdo->prepare("SELECT company_id FROM tiktok_accounts WHERE id = ?");
        $stmt->execute([$account_id]);
        $acc = $stmt->fetch();
        if (!$acc) {
            echo json_encode(['ok' => false, 'msg' => '账号不存在']);
            exit;
        }
        if (!$is_super_admin && intval($acc['company_id']) !== intval($company_id)) {
            echo json_encode(['ok' => false, 'msg' => '无权修改其他公司的账号']);
            exit;
        }
        // 超管修改其他公司时，确认与当前查看的公司匹配
        if ($is_super_admin && isset($_POST['company_id']) && intval($_POST['company_id']) > 0
            && intval($_POST['company_id']) !== intval($acc['company_id'])) {
            echo json_encode(['ok' => false, 'msg' => '账号不属于当前查看的公司']);
            exit;
        }

        $up = $pdo->prepare("UPDATE tiktok_accounts SET device_id = ? WHERE id = ?");
        $up->execute([$device_id, $account_id]);
        echo json_encode(['ok' => true, 'msg' => '保存成功', 'device_id' => $device_id]);
    } catch (Exception $e) {
        echo json_encode(['ok' => false, 'msg' => '保存失败:' . $e->getMessage()]);
    }
    exit;
}

// ========== 支持 URL 参数搜索 ==========
$url_search = isset($_GET['search']) ? trim($_GET['search']) : '';
$url_platform = isset($_GET['platform']) ? trim($_GET['platform']) : '';

// ========== IP检测函数（显示所有IP，国内+国际）==========
function getAllIPs() {
    $ipList = [];
    
    // 记录所有服务器变量（调试用）
    $all_headers = [];
    
    $headers = [
        'HTTP_CF_CONNECTING_IP',
        'HTTP_X_REAL_IP',
        'HTTP_X_FORWARDED_FOR',
        'HTTP_X_FORWARDED',
        'HTTP_X_CLUSTER_CLIENT_IP',
        'HTTP_CLIENT_IP',
        'HTTP_X_FORWARDED_FOR_AWS',
        'HTTP_X_ARR_CLIENT_IP',
        'HTTP_X_AZURE_CLIENTIP',
        'HTTP_FASTLY_CLIENT_IP',
        'HTTP_TRUE_CLIENT_IP',
        'HTTP_ALI_CDN_REAL_IP',
        'HTTP_TENCENT_CLIENT_IP',
        'HTTP_HW_CLIENT_IP',
        'HTTP_BAIDU_CLIENT_IP',
        'REMOTE_ADDR'
    ];
    
    foreach ($headers as $key) {
        if (!empty($_SERVER[$key])) {
            $value = trim($_SERVER[$key]);
            $all_headers[$key] = $value;  // 记录原始值
            
            if (strpos($value, ',') !== false) {
                $ips = array_map('trim', explode(',', $value));
                foreach ($ips as $ip) {
                    if (!empty($ip) && !in_array($ip, $ipList) && filter_var($ip, FILTER_VALIDATE_IP)) {
                        $ipList[] = $ip;
                    }
                }
            } else {
                if (!empty($value) && !in_array($value, $ipList) && filter_var($value, FILTER_VALIDATE_IP)) {
                    $ipList[] = $value;
                }
            }
        }
    }
    
    // 记录所有检测到的头部信息到日志
    logIPDetect("检测到的HTTP头", $all_headers);
    logIPDetect("解析出的IP列表", $ipList);
    
    // 去重
    $ipList = array_values(array_unique($ipList));
    
    if (empty($ipList)) {
        logIPDetect("警告：未检测到任何IP地址");
        $ipList[] = 'unknown';
    }
    
    return $ipList;
}

$all_ips = getAllIPs();

// 获取所有公司列表（供超级管理员选择）
$companies = [];
if ($is_super_admin) {
    $stmt = $pdo->query("SELECT id, company_name, status FROM companies WHERE status = 1 ORDER BY company_name");
    $companies = $stmt->fetchAll();
}

// 超级管理员可以选择公司，普通管理员只能看自己公司
$selected_company_id = $company_id;
if ($is_super_admin && isset($_GET['company_id']) && is_numeric($_GET['company_id'])) {
    $selected_company_id = intval($_GET['company_id']);
}

// 获取选中公司的账号列表
$accounts = [];
if ($selected_company_id > 0) {
    $sql = "
        SELECT
            a.id,
            a.login_account,
            a.platform,
            a.device_id,
            a.login_password,
            a.bind_email,
            a.email_password,
            a.safe_link,
            a.bind_phone
        FROM tiktok_accounts a
        WHERE a.company_id = ? AND a.status = 1
    ";
    $stmt = $pdo->prepare($sql);
    $stmt->execute([$selected_company_id]);
    $accounts = $stmt->fetchAll();
    
    // ========== 排序：按手机编号排序，没有手机编号的放在最后 ==========
    usort($accounts, function($a, $b) {
        $device_a = trim($a['device_id'] ?? '');
        $device_b = trim($b['device_id'] ?? '');
        
        // 判断是否为空
        $a_empty = empty($device_a);
        $b_empty = empty($device_b);
        
        // 空值排在最后
        if ($a_empty && !$b_empty) return 1;
        if (!$a_empty && $b_empty) return -1;
        if ($a_empty && $b_empty) return 0;
        
        // 提取数字进行数值排序（支持如 "手机001", "设备12" 等格式）
        preg_match_all('/\d+/', $device_a, $a_numbers);
        preg_match_all('/\d+/', $device_b, $b_numbers);
        
        $a_num = isset($a_numbers[0][0]) ? intval($a_numbers[0][0]) : 0;
        $b_num = isset($b_numbers[0][0]) ? intval($b_numbers[0][0]) : 0;
        
        if ($a_num != $b_num) {
            return $a_num - $b_num;
        }
        
        // 如果数字相同，按字符串排序
        return strcmp($device_a, $device_b);
    });
}

// 获取选中公司名称
$selected_company_name = '';
if ($is_super_admin && $selected_company_id) {
    $stmt = $pdo->prepare("SELECT company_name FROM companies WHERE id = ?");
    $stmt->execute([$selected_company_id]);
    $comp = $stmt->fetch();
    $selected_company_name = $comp ? $comp['company_name'] : '';
}
?>
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0, user-scalable=yes, viewport-fit=cover">
<title>快捷复制面板 - 管理员专用</title>
<style>
*{margin:0;padding:0;box-sizing:border-box;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif}
body{background:#f5f7fa;padding:12px}
.ip-info{text-align:center;font-size:13px;color:#555;margin-bottom:10px;padding:8px;background:#eef6ff;border-radius:10px;border:1px solid #d1e7ff;word-break:break-all}
.ip-info .ip-title{font-weight:600;color:#007aff;margin-right:8px}
.ip-info .ip-list{font-family:monospace}
.user-info{text-align:center;font-size:12px;color:#888;margin-bottom:10px;padding:5px}
.nav-avatar{width:20px;height:20px;border-radius:50%;object-fit:cover;vertical-align:middle;margin-right:4px;border:1px solid #ddd}
.nav-avatar-fallback{vertical-align:middle;margin-right:2px}
.company-selector{background:#fff;border-radius:12px;padding:12px;margin-bottom:15px;box-shadow:0 1px 3px rgba(0,0,0,0.08);display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.company-selector label{font-size:14px;font-weight:500;color:#333}
.company-selector select{flex:1;padding:10px 12px;border:1px solid #ddd;border-radius:10px;font-size:14px;background:#fff;min-width:150px}
.title{text-align:center;font-size:18px;font-weight:600;margin-bottom:15px;color:#333}

/* 搜索框样式 */
.search-bar {
    background: #fff;
    border-radius: 12px;
    padding: 12px;
    margin-bottom: 15px;
    box-shadow: 0 1px 3px rgba(0,0,0,0.08);
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
}
.search-bar label {
    font-size: 13px;
    color: #666;
    font-weight: 500;
}
.search-bar input {
    flex: 1;
    padding: 10px 14px;
    border: 1px solid #ddd;
    border-radius: 10px;
    font-size: 14px;
    background: #f8f9fc;
    outline: none;
    transition: all 0.2s;
    min-width: 180px;
}
.search-bar input:focus {
    border-color: #007aff;
    background: #fff;
    box-shadow: 0 0 0 2px rgba(0,122,255,0.1);
}
.search-bar .search-icon {
    font-size: 14px;
    color: #007aff;
}
.search-bar .clear-btn {
    background: #f0f0f0;
    border: none;
    padding: 8px 16px;
    border-radius: 10px;
    cursor: pointer;
    font-size: 13px;
    color: #666;
    transition: all 0.2s;
}
.search-bar .clear-btn:hover {
    background: #e0e0e0;
}
.search-count {
    font-size: 12px;
    color: #888;
    margin-left: auto;
}

.item{background:#fff;border-radius:14px;padding:15px;margin-bottom:12px;box-shadow:0 2px 8px rgba(0,0,0,0.06);border:1px solid #f0f0f0}
.num{font-size:15px;font-weight:600;margin-bottom:10px;color:#007aff;padding-bottom:5px;border-bottom:1px solid #f0f0f0;display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:8px}
.num-tag{display:inline-block;background:#e8eaf6;color:#3949ab;padding:4px 10px;border-radius:20px;font-size:12px;font-weight:normal}
.qr-btn{margin-left:auto;background:#007aff;color:#fff;border:none;padding:5px 14px;border-radius:20px;font-size:12px;cursor:pointer;transition:all .2s;white-space:nowrap}
.qr-btn:hover{background:#005bbf;transform:translateY(-1px)}
.qr-btn:active{transform:translateY(0)}
.qr-modal{display:none;position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,.55);z-index:9998;align-items:center;justify-content:center;padding:20px}
.qr-modal.show{display:flex}
.qr-modal-card{background:#fff;border-radius:16px;padding:24px;max-width:420px;width:100%;box-shadow:0 10px 40px rgba(0,0,0,.25);text-align:center;position:relative;max-height:90vh;overflow-y:auto}
.qr-modal-close{position:absolute;top:10px;right:14px;font-size:22px;line-height:1;cursor:pointer;color:#999;background:none;border:none}
.qr-modal-close:hover{color:#333}
.qr-modal-title{font-size:16px;font-weight:600;color:#333;margin-bottom:4px}
.qr-modal-sub{font-size:12px;color:#999;margin-bottom:16px;word-break:break-all}
.qr-canvas-wrap{background:#fff;border:1px solid #eee;border-radius:12px;padding:12px;display:inline-block;margin-bottom:16px}
.qr-code-img{display:block;width:220px;height:220px;image-rendering:pixelated}
.qr-field{display:flex;align-items:center;gap:8px;margin-bottom:10px;text-align:left;background:#f8f9fc;border-radius:10px;padding:10px 12px}
.qr-field .qf-label{width:48px;font-size:12px;color:#888;flex-shrink:0}
.qr-field .qf-value{flex:1;font-size:13px;color:#333;word-break:break-all}
.qr-field .qf-copy{background:#eef6ff;border:1px solid #b3d7ff;color:#007aff;border-radius:8px;padding:5px 12px;font-size:12px;cursor:pointer;flex-shrink:0}
.qr-field .qf-copy:hover{background:#dcecff}
.qr-tip{font-size:11px;color:#aaa;margin-top:12px}
.qr-grid{display:flex;gap:70px;justify-content:center;align-items:flex-start;flex-wrap:wrap;margin:0 auto}
.qr-cell{flex:0 0 auto;width:150px;background:#fafbfc;border:1px solid #eef2f6;border-radius:12px;padding:14px 12px}
.qr-cell-title{font-size:13px;font-weight:600;color:#333;margin-bottom:10px}
.qr-canvas-wrap{background:#fff;border:14px solid #fff;border-radius:10px;padding:4px;display:inline-block;margin-bottom:10px;box-shadow:0 0 0 1px #eee}
.qr-code-img{display:block;width:100px;height:100px;image-rendering:pixelated}
.qr-copy-btn{background:#eef6ff;border:1px solid #b3d7ff;color:#007aff;border-radius:8px;padding:6px 0;width:100%;font-size:12px;cursor:pointer}
.qr-copy-btn:hover{background:#dcecff}
.device-edit{display:inline-flex;align-items:center;gap:6px}
.device-edit .device-label{display:inline-block;background:#e8eaf6;color:#3949ab;padding:4px 10px;border-radius:20px;font-size:12px;font-weight:normal;cursor:pointer;transition:all .2s}
.device-edit .device-label:hover{background:#d3d9f7}
.device-edit .device-label .dev-val{font-weight:600}
.device-edit .device-actions{display:inline-flex;align-items:center;gap:4px}
.device-edit .dev-input{width:90px;padding:4px 8px;border:1px solid #b3d7ff;border-radius:8px;font-size:12px;outline:none}
.device-edit .dev-input:focus{border-color:#007aff;box-shadow:0 0 0 2px rgba(0,122,255,.12)}
.device-edit .dev-btn{padding:4px 10px;border:none;border-radius:8px;font-size:11px;cursor:pointer}
.device-edit .dev-btn-save{background:#52c41a;color:white}
.device-edit .dev-btn-save:hover{background:#389e0d}
.device-edit .dev-btn-cancel{background:#f0f0f0;color:#666}
.device-edit .dev-btn-cancel:hover{background:#e0e0e0}
.line{display:flex;margin-bottom:8px;align-items:center;flex-wrap:wrap}
.label{width:70px;font-size:13px;color:#888;flex-shrink:0}
.text{flex:1;padding:10px 12px;background:#f8f9fc;border-radius:10px;font-size:14px;cursor:pointer;word-break:break-all;border:1px solid #eef2f6}
.text:hover{background:#eef6ff;border-color:#007aff}
.sms-box{margin-top:12px;padding:12px;background:#f6ffed;border-radius:10px;border:1px solid #b7eb8f;text-align:center}
.sms-title{font-size:13px;color:#237804;margin-bottom:6px}
.sms-code{font-size:28px;font-weight:bold;color:#389e0d;letter-spacing:4px;cursor:pointer;font-family:monospace}
.sms-btn{background:#52c41a;color:white;border:none;padding:6px 16px;border-radius:6px;font-size:12px;cursor:pointer;margin-left:10px}
.sms-btn:hover{background:#389e0d}
.sms-btn:disabled{background:#ccc;cursor:not-allowed}
.totp-box{margin-top:12px;padding:12px;background:#f0f9ff;border-radius:10px;border:1px solid #bae7ff;text-align:center}
.totp-title{font-size:13px;color:#0050b3;margin-bottom:6px}
.totp-code{font-size:28px;font-weight:bold;color:#1890ff;letter-spacing:4px;cursor:pointer;font-family:monospace}
.totp-retry-btn{background:#ff9800;color:white;border:none;padding:6px 12px;border-radius:6px;font-size:12px;cursor:pointer;transition:all 0.2s}
.totp-retry-btn:hover{background:#e65100;transform:translateY(-1px)}
.empty-state{text-align:center;padding:40px;color:#999;background:#fff;border-radius:14px}
.admin-badge{background:#667eea;color:white;font-size:11px;padding:2px 10px;border-radius:20px;margin-left:10px}
#toast{position:fixed;top:50%;left:50%;transform:translate(-50%,-50%);background:rgba(0,0,0,.75);color:#fff;padding:10px 18px;border-radius:30px;font-size:14px;z-index:9999;opacity:0;pointer-events:none;transition:opacity .2s;white-space:nowrap}
#toast.show{opacity:1}
.item-hidden{display:none !important}
@media (max-width:480px){
    body{padding:10px}
    .label{width:65px;font-size:12px}
    .text{padding:8px 10px;font-size:13px}
    .totp-code{font-size:24px;letter-spacing:2px}
    .sms-code{font-size:24px;letter-spacing:2px}
    .item{padding:12px}
    .search-bar input{min-width:120px}
    .ip-info{font-size:11px}
    .qr-modal-card{max-width:92vw;padding:18px}
    .qr-grid{flex-direction:column;gap:32px}
    .qr-cell{width:100%;max-width:240px;margin:0 auto}
}
</style>
<!-- 本地二维码生成库（qrcode-generator，支持中文/UTF-8） -->
<script src="assets/qrcode.min.js"></script>
</head>
<body>
<div id="toast"></div>

<!-- 扫码复制二维码模态框 -->
<div class="qr-modal" id="qr_modal" onclick="if(event.target===this)closeQrModal()">
    <div class="qr-modal-card">
        <button class="qr-modal-close" onclick="closeQrModal()">×</button>
        <div class="qr-modal-title">扫码单独复制</div>
        <div class="qr-modal-sub" id="qr_account_name"></div>
        <div class="qr-grid">
            <div class="qr-cell">
                <div class="qr-cell-title">👤 用户名</div>
                <div class="qr-canvas-wrap">
                    <img id="qr_img_name" class="qr-code-img" alt="用户名二维码" src="" />
                </div>
                <button class="qr-copy-btn" onclick="copyRaw('用户名')">复制用户名</button>
            </div>
            <div class="qr-cell">
                <div class="qr-cell-title">🔑 密码</div>
                <div class="qr-canvas-wrap">
                    <img id="qr_img_pass" class="qr-code-img" alt="密码二维码" src="" />
                </div>
                <button class="qr-copy-btn" onclick="copyRaw('密码')">复制密码</button>
            </div>
        </div>
        <div class="qr-tip">📷 微信/支付宝/相机分别扫两个二维码，单独复制</div>
    </div>
</div>

<div class="user-info">
    <?php if(!empty($user['dingtalk_avatar'])): ?><img src="<?php echo htmlspecialchars($user['dingtalk_avatar']); ?>" class="nav-avatar" alt="" onerror="this.style.display='none';this.nextElementSibling.style.display='inline'"><?php endif; ?><span class="nav-avatar-fallback" style="<?php echo !empty($user['dingtalk_avatar']) ? 'display:none' : ''; ?>">👤</span> <?php echo htmlspecialchars($user['real_name']); ?> (<?php echo $user['role']; ?>)
    <?php if($is_super_admin): ?>
    <span class="admin-badge">超级管理员</span>
    <?php endif; ?>
</div>

<div class="ip-info">
    📡 国内IP：<?php echo implode(' | ', $all_ips); ?>
</div>

<div class="ip-info" id="ip_info">
    🌍 国际IP：<span id="international_ip">检测中...</span>
</div>

<script>
// 国际IP检测（带超时，避免拖慢首屏）
(function() {
    const el = document.getElementById('international_ip');
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 6000);
    fetch('https://api.ip.sb/ip', { signal: controller.signal })
        .then(res => res.text())
        .then(ip => { el.innerHTML = (ip || '').trim() || '未开启VPN'; })
        .catch(() => { if (el) el.innerHTML = '未开启VPN'; })
        .finally(() => clearTimeout(timer));
})();
</script>

<?php
// 生成账号数据供 JS 使用（用于扫码复制二维码）
$account_data = [];
foreach ($accounts as $acc) {
    $account_data[] = [
        'id' => $acc['id'],
        'name' => trim($acc['login_account'] ?? ''),
        'password' => (string)($acc['login_password'] ?? ''),
    ];
}
?>
<script>
// 账号数据表（按ID索引，供扫码复制使用）
window.ACCOUNT_DATA = <?php echo json_encode($account_data, JSON_UNESCAPED_UNICODE); ?>.reduce((m, a) => { m[a.id] = a; return m; }, {});
</script>

<?php if($is_super_admin && count($companies) > 0): ?>
<div class="company-selector">
    <label>🏢 切换公司：</label>
    <form method="GET" style="flex:1; display:flex; gap:10px;">
        <select name="company_id" onchange="this.form.submit()">
            <?php foreach($companies as $comp): ?>
            <option value="<?php echo $comp['id']; ?>" <?php echo ($selected_company_id == $comp['id']) ? 'selected' : ''; ?>>
                <?php echo htmlspecialchars($comp['company_name']); ?>
            </option>
            <?php endforeach; ?>
        </select>
    </form>
    <?php if($selected_company_id != $company_id): ?>
    <span style="color:#ff9800; font-size:12px;">⚠️ 正在查看其他公司</span>
    <?php endif; ?>
</div>
<?php endif; ?>

<div class="title">
    📋 账号密码快捷复制
    <?php if($selected_company_name): ?>
    <span style="font-size:12px; color:#888;"> - <?php echo htmlspecialchars($selected_company_name); ?></span>
    <?php endif; ?>
</div>

<!-- 搜索过滤区域 -->
<div class="search-bar">
    <span class="search-icon">🔍</span>
    <input type="text" id="searchInput" placeholder="搜索手机编号 或 登录账号..." autocomplete="off" value="<?php echo htmlspecialchars($url_search); ?>">
    <button class="clear-btn" onclick="clearSearch()">✖️ 清空</button>
    <span class="search-count" id="searchCount"></span>
</div>

<select id="platformFilter" onchange="filterAccounts()" style="padding:8px 12px; border:1px solid #ddd; border-radius:10px; background:#fff;">
    <option value="">全部平台</option>
    <option value="tiktok" <?php echo $url_platform == 'tiktok' ? 'selected' : ''; ?>>🎵 TikTok</option>
    <option value="youtube" <?php echo $url_platform == 'youtube' ? 'selected' : ''; ?>>▶️ YouTube</option>
</select>

<?php if(empty($accounts)): ?>
<div class="empty-state">
    <div style="font-size:48px; margin-bottom:12px">📭</div>
    <p>该公司暂无账号数据</p>
</div>
<?php else: ?>
    <?php foreach($accounts as $idx => $acc): ?>
<div class="item" data-id="<?php echo $acc['id']; ?>" 
     data-platform="<?php echo htmlspecialchars($acc['platform'] ?? ''); ?>"
     data-device="<?php echo htmlspecialchars(strtolower($acc['device_id'] ?? '')); ?>" 
     data-name="<?php echo htmlspecialchars(strtolower(trim($acc['login_account'] ?? ''))); ?>">

<div class="num">
    <span>账号 <span class="item-index"><?php echo $idx + 1; ?></span></span>
    <?php 
    // 平台图标和文字
    $platform_icon = ($acc['platform'] == 'youtube') ? '▶️' : '🎵';
    $platform_name = ($acc['platform'] == 'youtube') ? 'YouTube' : 'TikTok';
    $platform_color = ($acc['platform'] == 'youtube') ? '#ff0000' : '#000000';
    ?>
    <span class="num-tag" style="background:<?php echo $platform_color; ?>20; color:<?php echo $platform_color; ?>;">
        <?php echo $platform_icon; ?> <?php echo $platform_name; ?>
    </span>
    <span class="device-edit" data-id="<?php echo $acc['id']; ?>">
        <span class="device-label" onclick="editDevice(this)" title="点击修改手机编号">
            📱 <span class="dev-val"><?php echo !empty($acc['device_id']) ? htmlspecialchars($acc['device_id']) : '未设置'; ?></span> ✏️
        </span>
        <span class="device-actions" style="display:none;">
            <input type="text" class="dev-input" value="<?php echo htmlspecialchars($acc['device_id'] ?? ''); ?>" placeholder="输入手机编号" maxlength="50" />
            <button class="dev-btn dev-btn-save" onclick="saveDevice(this)">保存</button>
            <button class="dev-btn dev-btn-cancel" onclick="cancelDeviceEdit(this)">取消</button>
        </span>
    </span>
    <button class="qr-btn" data-id="<?php echo $acc['id']; ?>" onclick="openQrModal('<?php echo $acc['id']; ?>')">📲 扫码复制</button>
</div>
        
        <div class="line">
            <div class="label">登录账号</div>
            <div class="text" onclick="copyText(this)"><?php $la = trim($acc['login_account'] ?? ''); echo $la !== '' ? htmlspecialchars($la) : '-'; ?></div>
        </div>
        
        <div class="line">
            <div class="label">密码</div>
            <div class="text" onclick="copyText(this)"><?php echo htmlspecialchars($acc['login_password'] ?: '未设置'); ?></div>
        </div>
        
        <div class="line">
            <div class="label">邮箱</div>
            <div class="text" onclick="copyText(this)"><?php echo htmlspecialchars($acc['bind_email'] ?: '未设置'); ?></div>
        </div>
        
        <div class="line">
            <div class="label">邮箱密码</div>
            <div class="text" onclick="copyText(this)"><?php echo htmlspecialchars($acc['email_password'] ?: '未设置'); ?></div>
        </div>
        
        <div class="line">
            <div class="label">安全链接</div>
            <div class="text" onclick="copyText(this)" style="color:#0066cc; font-size:12px;"><?php echo htmlspecialchars($acc['safe_link'] ?: '未设置'); ?></div>
        </div>
        
        <!-- 手机验证码区域 -->
        <?php if(!empty($acc['bind_phone'])): ?>
        <div class="sms-box">
            <div class="sms-title">📱 手机验证码</div>
            <div class="sms-code" id="sms_code_<?php echo $acc['id']; ?>" data-phone="<?php echo htmlspecialchars($acc['bind_phone']); ?>" onclick="copyText(this)">点击获取</div>
            <button class="sms-btn" onclick="getSMSCode('<?php echo $acc['id']; ?>', this)">获取验证码</button>
        </div>
        <?php endif; ?>
        
        <!-- TOTP验证码区域（安全链接） -->
        <?php if(!empty($acc['safe_link'])): ?>
        <div class="totp-box" id="totp_box_<?php echo $acc['id']; ?>">
            <div class="totp-title">⏰ 验证码（30秒自动刷新）</div>
            <div class="totp-code-wrapper" style="display: flex; align-items: center; justify-content: center; gap: 10px; flex-wrap: wrap;">
                <div class="totp-code" id="totp_code_<?php echo $acc['id']; ?>" data-url="<?php echo htmlspecialchars($acc['safe_link']); ?>" onclick="copyText(this)">加载中...</div>
                <button class="totp-retry-btn" data-account-id="<?php echo $acc['id']; ?>" onclick="retryTOTPCode('<?php echo $acc['id']; ?>', this)" style="display: none;">🔄 重新获取</button>
            </div>
        </div>
        <?php endif; ?>
    </div>
    <?php endforeach; ?>
<?php endif; ?>

<script>
// ========== 扫码复制二维码功能 ==========
// 依赖 head 中的 assets/qrcode.min.js（本地库，离线可用）

// 渲染单个二维码图片（内容为纯文本）
function renderQr(imgEl, text) {
    if (typeof qrcode === 'undefined') {
        imgEl.alt = '二维码库加载失败';
        toast('二维码库加载失败');
        return;
    }
    const content = text || '';
    if (!content) {
        imgEl.removeAttribute('src');
        imgEl.alt = '（空）';
        return;
    }
    try {
        const qr = qrcode(0, 'M'); // 0 = 自动选择版本
        qr.addData(content, 'Byte');
        qr.make();
        imgEl.src = qr.createDataURL(4, 6); // cellSize=4, margin=6
        imgEl.alt = '二维码';
    } catch (e) {
        console.error('生成二维码失败:', e);
        imgEl.removeAttribute('src');
        imgEl.alt = '生成失败';
        toast('生成二维码失败');
    }
}

// 打开扫码复制模态框（用户名、密码分别生成二维码）
function openQrModal(accountId) {
    const account = (window.ACCOUNT_DATA || {})[accountId];
    if (!account) {
        toast('未找到账号数据');
        return;
    }
    const name = account.name || '';
    const pass = account.password || '';
    document.getElementById('qr_account_name').textContent =
        '账号：' + (name || '（无用户名）');

    // 分别生成两个二维码
    renderQr(document.getElementById('qr_img_name'), name);
    renderQr(document.getElementById('qr_img_pass'), pass);

    window.__qrAccount = { name: name, password: pass };
    document.getElementById('qr_modal').classList.add('show');
}

// 复制模态框中的某个字段（用户名/密码）
function copyRaw(label) {
    const acc = window.__qrAccount;
    if (!acc) return;
    const v = label === '密码' ? (acc.password || '') : (acc.name || '');
    if (!v) { toast('无内容可复制'); return; }
    // 复制到剪贴板
    if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(v).then(() => toast('复制成功')).catch(() => fallbackCopy(v));
    } else {
        fallbackCopy(v);
    }
}

// 兜底复制（兼容老浏览器）
function fallbackCopy(text) {
    const textarea = document.createElement('textarea');
    textarea.value = text;
    textarea.style.position = 'fixed';
    textarea.style.opacity = '0';
    document.body.appendChild(textarea);
    textarea.select();
    try { document.execCommand('copy'); toast('复制成功'); }
    catch (e) { toast('复制失败'); }
    document.body.removeChild(textarea);
}

// 关闭扫码复制模态框
function closeQrModal() {
    document.getElementById('qr_modal').classList.remove('show');
    window.__qrAccount = null;
}

// 支持 ESC 关闭
document.addEventListener('keydown', function(e) {
    if (e.key === 'Escape') closeQrModal();
});

// 进入编辑手机编号状态
function editDevice(labelEl) {
    const wrap = labelEl.closest('.device-edit');
    if (!wrap) return;
    const label = wrap.querySelector('.device-label');
    const actions = wrap.querySelector('.device-actions');
    const input = wrap.querySelector('.dev-input');
    label.style.display = 'none';
    actions.style.display = 'inline-flex';
    input.focus();
    input.select();
    // 保存当前值，便于取消恢复
    wrap.dataset.orig = input.value;
}
// 取消编辑
function cancelDeviceEdit(btn) {
    const wrap = btn.closest('.device-edit');
    if (!wrap) return;
    const label = wrap.querySelector('.device-label');
    const actions = wrap.querySelector('.device-actions');
    const input = wrap.querySelector('.dev-input');
    input.value = wrap.dataset.orig || '';
    actions.style.display = 'none';
    label.style.display = 'inline-block';
}
// 保存手机编号
async function saveDevice(btn) {
    const wrap = btn.closest('.device-edit');
    if (!wrap) return;
    const accountId = wrap.dataset.id;
    const input = wrap.querySelector('.dev-input');
    const deviceVal = (input.value || '').trim();

    if (!accountId) { toast('缺少账号ID'); return; }

    btn.disabled = true;
    btn.innerText = '保存中...';
    try {
        const body = new URLSearchParams();
        body.append('action', 'update_device');
        body.append('id', accountId);
        body.append('device_id', deviceVal);
        const res = await fetch(window.location.pathname, {
            method: 'POST',
            headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
            body: body.toString()
        });
        const data = await res.json();
        if (data.ok) {
            toast('保存成功');
            // 刷新页面以应用新的手机编号排序
            setTimeout(() => location.reload(), 500);
        } else {
            toast(data.msg || '保存失败');
            btn.disabled = false;
            btn.innerText = '保存';
        }
    } catch (e) {
        console.error('保存手机编号失败:', e);
        toast('保存失败，请重试');
        btn.disabled = false;
        btn.innerText = '保存';
    }
}

// 输入框回车保存
document.addEventListener('keydown', function(e) {
    if (e.key === 'Enter' && e.target && e.target.classList && e.target.classList.contains('dev-input')) {
        const wrap = e.target.closest('.device-edit');
        if (wrap) {
            const saveBtn = wrap.querySelector('.dev-btn-save');
            if (saveBtn) saveDevice(saveBtn);
        }
    }
});

 function copyText(el) {
 
     let text = el.innerText.trim();
    if (text === '未设置' || text === '加载中...' || text === '点击获取' || text === '获取失败' || text === '请求失败') {
        toast('无内容可复制');
        return;
    }
    
    const input = document.createElement('input');
    input.value = text;
    document.body.appendChild(input);
    input.select();
    document.execCommand('copy');
    document.body.removeChild(input);
    toast('复制成功');
}

// 轻提示
function toast(msg) {
    const t = document.getElementById('toast');
    t.innerText = msg;
    t.classList.add('show');
    setTimeout(() => t.classList.remove('show'), 1200);
}

// 获取手机验证码
async function getSMSCode(accountId, btn) {
    const codeDiv = document.getElementById('sms_code_' + accountId);
    const phone = codeDiv.getAttribute('data-phone');
    
    if (!phone) {
        toast('未设置手机号');
        return;
    }
    
    // 禁用按钮，防止重复点击
    btn.disabled = true;
    btn.innerText = '发送中...';
    codeDiv.innerText = '发送中...';
    
    try {
        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), 10000);
        
        // 调用短信 API
        const smsUrl = 'http://umlmail.site/Mail/GetCodeSMS?token=7QR2zD6Fqw05261315';
        const res = await fetch(smsUrl, { signal: controller.signal });
        clearTimeout(timeoutId);
        
        const text = await res.text();
        
        // 匹配6位数字验证码
        const codeMatch = text.match(/\d{6}/);
        if (codeMatch) {
            codeDiv.innerText = codeMatch[0];
            toast('验证码获取成功');
        } else {
            codeDiv.innerText = '获取失败';
            toast('获取失败，请重试');
        }
    } catch(e) {
        console.error('获取验证码失败:', e);
        codeDiv.innerText = '请求失败';
        toast('请求失败，请检查网络');
    } finally {
        // 60秒后才能再次点击
        let countdown = 60;
        btn.disabled = true;
        btn.innerText = countdown + '秒后重试';
        
        const interval = setInterval(() => {
            countdown--;
            if (countdown <= 0) {
                clearInterval(interval);
                btn.disabled = false;
                btn.innerText = '获取验证码';
                if (codeDiv.innerText === '发送中...' || codeDiv.innerText === '请求失败') {
                    codeDiv.innerText = '点击获取';
                }
            } else {
                btn.innerText = countdown + '秒后重试';
            }
        }, 1000);
    }
}

// ========== TOTP 并发控制与懒加载 ==========

// 全局并发池：控制同时进行的 TOTP 请求数量，避免浏览器连接数被占满
const TOTP_MAX_CONCURRENCY = 6;
let totpQueue = [];
let totpActive = 0;

// 请求级缓存：短时间(10秒)内同一账号不重复请求，避免定时器叠加造成的堆积
const codeCache = new Map();

// 每个 TOTP 元素当前是否正在请求（防止同一元素重复触发）
const totpBusy = new WeakSet();

function totpNextWorker() {
    while (totpActive < TOTP_MAX_CONCURRENCY && totpQueue.length > 0) {
        const task = totpQueue.shift();
        totpActive++;
        task().finally(() => {
            totpActive--;
            totpNextWorker();
        });
    }
}

function totpEnqueue(task) {
    totpQueue.push(task);
    totpNextWorker();
}

// 获取单个TOTP验证码
async function doFetchTOTP(url) {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 8000);
    try {
        const res = await fetch(url, { signal: controller.signal });
        const text = await res.text();
        return text.match(/\d{6}/) ? text.match(/\d{6}/)[0] : null;
    } finally {
        clearTimeout(timeoutId);
    }
}

// 更新界面元素（不占用请求并发，仅负责渲染结果）
function renderTOTPResult(ele, code) {
    const accountId = ele.id ? ele.id.replace('totp_code_', '') : null;
    const retryBtn = accountId ? document.querySelector(`.totp-retry-btn[data-account-id="${accountId}"]`) : null;
    if (code) {
        ele.innerText = code;
        ele.style.color = '#1890ff';
        if (retryBtn) retryBtn.style.display = 'none';
        // 缓存成功结果：验证码30秒有效，缓存25秒防止不必要的重复请求
        codeCache.set(ele.dataset.url, { code, time: Date.now() });
    } else {
        // 失败时保留原本内容，仅展示重试按钮，避免把页面上已显示的验证码刷掉
        ele.style.color = ele.innerText === '加载中...' ? '#ff4d4f' : ele.style.color;
        if (retryBtn) retryBtn.style.display = 'inline-block';
    }
}

// 带并发控制的TOTP获取入口
function getTOTPCode(ele, showRetryBtn = true) {
    return new Promise((resolve) => {
        const url = ele.dataset.url;
        if (!url || url === '未设置') {
            ele.innerText = '无链接';
            resolve(false);
            return;
        }
        // 命中缓存直接返回
        const cache = codeCache.get(url);
        if (cache && (Date.now() - cache.time) < TOTP_CACHE_WINDOW) {
            ele.innerText = cache.code;
            ele.style.color = '#1890ff';
            resolve(true);
            return;
        }
        // 同一元素正在请求则跳过
        if (totpBusy.has(ele)) {
            resolve(false);
            return;
        }
        totpBusy.add(ele);
        totpEnqueue(async () => {
            try {
                const code = await doFetchTOTP(url);
                renderTOTPResult(ele, code);
                resolve(!!code);
            } finally {
                totpBusy.delete(ele);
            }
        });
    });
}

// 重试获取验证码
async function retryTOTPCode(accountId, btn) {
    const codeDiv = document.getElementById('totp_code_' + accountId);
    if (!codeDiv) return;
    // 直接重试，不清空当前显示的验证码
    btn.disabled = true;
    btn.innerText = '⏳ 重试中...';
    const success = await getTOTPCode(codeDiv, true);
    btn.disabled = false;
    btn.innerText = '🔄 重新获取';
    if (success) {
        btn.style.display = 'none';
    }
}

// 刷新所有TOTP（并行 + 并发受限，不再相互阻塞）
const TOTP_REFRESH_INTERVAL = 30000;
const TOTP_CACHE_WINDOW = 25000; // 缓存窗口略小于刷新间隔

let autoRefreshTimer = null;
let refreshing = false;
let refreshTick = 0;

function refreshAllTOTP() {
    // 防止定时器与手动触发重叠
    if (refreshing) return Promise.resolve();
    refreshing = true;
    refreshTick++;

    const els = [];
    document.querySelectorAll('.totp-code').forEach(el => {
        if (el.dataset.url && el.dataset.url !== '未设置') {
            // 只刷新已加载过/可见的，减少无效请求
            if (el.innerText !== '加载中...') {
                els.push(el);
            }
        }
    });

    // 全部并发提交到并发池
    return Promise.all(els.map(el => getTOTPCode(el, true).catch(() => false))).finally(() => {
        refreshing = false;
    });
}

function startAutoRefresh() {
    if (autoRefreshTimer) clearInterval(autoRefreshTimer);
    autoRefreshTimer = setInterval(() => {
        refreshAllTOTP();
    }, TOTP_REFRESH_INTERVAL);
}

// 清空搜索并清除 URL 参数
function clearSearch() {
    document.getElementById('searchInput').value = '';
    document.getElementById('platformFilter').value = '';
    filterAccounts();
    clearSearchUrl();
    document.getElementById('searchInput').focus();
}

// 仅清除搜索相关 URL 参数（保留 company_id 等其它参数）
function clearSearchUrl() {
    try {
        const url = new URL(window.location.href);
        url.searchParams.delete('search');
        url.searchParams.delete('platform');
        window.history.replaceState({}, '', url.toString());
    } catch (e) {}
}

// ========== 搜索过滤功能 ==========
function filterAccounts() {
    const searchTerm = document.getElementById('searchInput').value.trim().toLowerCase();
    const platformFilter = document.getElementById('platformFilter')?.value.toLowerCase() || '';
    const items = document.querySelectorAll('.item');
    let visibleCount = 0;
    let index = 1;
    
    items.forEach(item => {
        const deviceId = item.getAttribute('data-device') || '';
        const accountName = item.getAttribute('data-name') || '';
        const platform = item.getAttribute('data-platform') || '';
        
        // 检查是否匹配平台筛选
        let platformMatch = true;
        if (platformFilter !== '') {
            platformMatch = platform === platformFilter;
        }
        
        // 检查是否匹配搜索词
        let searchMatch = true;
        if (searchTerm !== '') {
            searchMatch = deviceId.includes(searchTerm) || accountName.includes(searchTerm);
        }
        
        if (platformMatch && searchMatch) {
            item.classList.remove('item-hidden');
            const indexSpan = item.querySelector('.item-index');
            if (indexSpan) {
                indexSpan.innerText = index;
            }
            visibleCount++;
            index++;
        } else {
            item.classList.add('item-hidden');
        }
    });
    
    const searchCount = document.getElementById('searchCount');
    if (searchCount) {
        const totalCount = items.length;
        if (searchTerm === '' && platformFilter === '') {
            searchCount.innerText = `共 ${totalCount} 个账号`;
        } else {
            searchCount.innerText = `找到 ${visibleCount} / ${totalCount} 个账号`;
        }
    }
}

// 把当前搜索词和平台同步到 URL（浏览器地址栏），刷新后可自动恢复
function syncSearchToUrl() {
    try {
        const input = document.getElementById('searchInput');
        const platform = document.getElementById('platformFilter');
        const url = new URL(window.location.href);
        const term = (input && input.value.trim()) || '';
        const plat = (platform && platform.value) || '';
        if (term) {
            url.searchParams.set('search', term);
        } else {
            url.searchParams.delete('search');
        }
        if (plat) {
            url.searchParams.set('platform', plat);
        } else {
            url.searchParams.delete('platform');
        }
        window.history.replaceState({}, '', url.toString());
    } catch (e) {}
}

// 监听输入事件（防抖，减少频繁 DOM 遍历，并同步到 URL）
let searchDebounceTimer = null;
document.getElementById('searchInput')?.addEventListener('input', function() {
    clearTimeout(searchDebounceTimer);
    searchDebounceTimer = setTimeout(() => {
        filterAccounts();
        syncSearchToUrl();
    }, 150);
});

// 监听平台筛选变化
document.getElementById('platformFilter')?.addEventListener('change', function() {
    filterAccounts();
    syncSearchToUrl();
});

// ========== 初始化（懒加载 + 并发刷新） ==========

// 只加载当前可视区域内的 TOTP，滚动到附近才触发加载
function lazyLoadVisibleTOTP() {
    const els = [];
    document.querySelectorAll('.totp-code').forEach(el => {
        if (!el.dataset.url || el.dataset.url === '未设置') return;
        if (el.dataset.loaded) return;       // 已加载过
        if (el.innerText !== '加载中...') return; // 已处于初始外状态时跳过
        const rect = el.getBoundingClientRect();
        // 视口上方或下方提前 300px 内，视为需要加载
        if (rect.bottom >= -300 && rect.top <= (window.innerHeight + 300)) {
            els.push(el);
        }
    });
    els.forEach(el => {
        el.dataset.loaded = '1';
        getTOTPCode(el, true).catch(() => {});
    });
}

let lazyObserver = null;
window.onload = () => {
    // 首次只加载首屏可见的 TOTP，页面立即可交互
    lazyLoadVisibleTOTP();

    // 滚动时按需加载更多，避免一次性并发几十个请求
    if ('IntersectionObserver' in window) {
        lazyObserver = new IntersectionObserver((entries) => {
            entries.forEach(entry => {
                if (entry.isIntersecting) {
                    const el = entry.target;
                    if (!el.dataset.loaded) {
                        el.dataset.loaded = '1';
                        getTOTPCode(el, true).catch(() => {});
                    }
                }
            });
        }, { rootMargin: '300px 0px' });
        document.querySelectorAll('.totp-code').forEach(el => {
            if (el.dataset.url && el.dataset.url !== '未设置') {
                lazyObserver.observe(el);
            }
        });
    } else {
        // 兼容不支持 IntersectionObserver 的浏览器
        window.addEventListener('scroll', () => lazyLoadVisibleTOTP(), { passive: true });
    }

    // 启动定时刷新（只刷新已加载过的，并发受限）
    startAutoRefresh();

    // 如果有 URL 参数，自动执行搜索
    filterAccounts();
};
</script>
</body>
</html>