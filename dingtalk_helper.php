<?php
// dingtalk_helper.php - 钉钉考勤状态获取辅助函数
// 被 bwbrowser_sync.php 引用

require_once __DIR__ . '/config.php';

/**
 * 获取钉钉配置
 */
function dingtalk_get_config($pdo, $company_id = 1) {
    $stmt = $pdo->prepare("SELECT * FROM salary_dingtalk_config WHERE company_id = ?");
    $stmt->execute([$company_id]);
    $config = $stmt->fetch();
    if (!$config) {
        $stmt = $pdo->prepare("SELECT * FROM salary_dingtalk_config WHERE company_id = 0");
        $stmt->execute();
        $config = $stmt->fetch();
    }
    return [
        'enabled' => isset($config['enabled']) ? intval($config['enabled']) : 1,
        'app_key' => $config['app_key'] ?? '',
        'app_secret' => $config['app_secret'] ?? '',
        'access_token' => $config['access_token'] ?? '',
        'token_expires_at' => $config['token_expires_at'] ?? '',
        'root_dept_id' => $config['root_dept_id'] ?? '1',
    ];
}

/**
 * 获取钉钉 AccessToken（带缓存）
 */
function dingtalk_get_token($pdo, $company_id = 1) {
    $config = dingtalk_get_config($pdo, $company_id);
    if (empty($config['app_key']) || empty($config['app_secret'])) {
        return null;
    }

    // 检查缓存 token 是否有效
    if (!empty($config['access_token']) && !empty($config['token_expires_at']) && strtotime($config['token_expires_at']) > time() + 60) {
        return $config['access_token'];
    }

    // 获取新 token
    $url = "https://oapi.dingtalk.com/gettoken?appkey=" . urlencode($config['app_key']) . "&appsecret=" . urlencode($config['app_secret']);
    $ch = curl_init($url);
    curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
    curl_setopt($ch, CURLOPT_TIMEOUT, 10);
    curl_setopt($ch, CURLOPT_SSL_VERIFYPEER, false);
    $resp = curl_exec($ch);
    curl_close($ch);

    $result = json_decode($resp, true);
    if (intval($result['errcode'] ?? -1) !== 0 || empty($result['access_token'])) {
        return null;
    }

    // 缓存 token
    $token = $result['access_token'];
    $expires = date('Y-m-d H:i:s', time() + intval($result['expires_in'] ?? 7200));
    $stmt = $pdo->prepare("UPDATE salary_dingtalk_config SET access_token = ?, token_expires_at = ? WHERE company_id = ?");
    $stmt->execute([$token, $expires, $company_id]);

    return $token;
}

/**
 * 获取所有钉钉用户列表
 */
function dingtalk_get_users($access_token, $root_dept_id = '1') {
    $users = [];

    // 获取部门列表
    $dept_ids = [$root_dept_id];
    $queue = [$root_dept_id];
    $seen = [$root_dept_id => true];

    while (!empty($queue)) {
        $dept_id = array_shift($queue);
        $url = "https://oapi.dingtalk.com/topapi/v2/department/listsub?access_token=" . urlencode($access_token);
        $payload = json_encode(['dept_id' => intval($dept_id), 'language' => 'zh_CN']);
        $ch = curl_init($url);
        curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
        curl_setopt($ch, CURLOPT_POST, true);
        curl_setopt($ch, CURLOPT_POSTFIELDS, $payload);
        curl_setopt($ch, CURLOPT_HTTPHEADER, ['Content-Type: application/json']);
        curl_setopt($ch, CURLOPT_TIMEOUT, 10);
        curl_setopt($ch, CURLOPT_SSL_VERIFYPEER, false);
        $resp = curl_exec($ch);
        curl_close($ch);

        $res = json_decode($resp, true);
        if (intval($res['errcode'] ?? -1) !== 0) continue;
        $children = $res['result'] ?? [];
        foreach ($children as $dept) {
            $child_id = (string)($dept['dept_id'] ?? '');
            if ($child_id !== '' && empty($seen[$child_id])) {
                $seen[$child_id] = true;
                $dept_ids[] = $child_id;
                $queue[] = $child_id;
            }
        }
    }

    // 获取每个部门的用户
    foreach ($dept_ids as $dept_id) {
        $cursor = 0;
        do {
            $url = "https://oapi.dingtalk.com/topapi/v2/user/list?access_token=" . urlencode($access_token);
            $payload = json_encode([
                'dept_id' => intval($dept_id),
                'cursor' => $cursor,
                'size' => 100,
                'order_field' => 'modify_desc',
                'contain_access_limit' => false,
                'language' => 'zh_CN'
            ]);
            $ch = curl_init($url);
            curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
            curl_setopt($ch, CURLOPT_POST, true);
            curl_setopt($ch, CURLOPT_POSTFIELDS, $payload);
            curl_setopt($ch, CURLOPT_HTTPHEADER, ['Content-Type: application/json']);
            curl_setopt($ch, CURLOPT_TIMEOUT, 10);
            curl_setopt($ch, CURLOPT_SSL_VERIFYPEER, false);
            $resp = curl_exec($ch);
            curl_close($ch);

            $res = json_decode($resp, true);
            if (intval($res['errcode'] ?? -1) !== 0) break;

            $list = $res['result']['list'] ?? [];
            foreach ($list as $item) {
                $userid = (string)($item['userid'] ?? '');
                $name = trim((string)($item['name'] ?? ''));
                if ($userid !== '' && $name !== '') {
                    $users[$userid] = [
                        'userid' => $userid,
                        'name' => $name,
                    ];
                }
            }

            $has_more = !empty($res['result']['has_more']);
            $cursor = intval($res['result']['next_cursor'] ?? 0);
        } while ($has_more && $cursor > 0);
    }

    return $users;
}

/**
 * 获取今日考勤状态
 * 返回: [real_name => checked_in(bool)]
 */
function dingtalk_get_today_attendance($pdo, $company_id = 1) {
    $token = dingtalk_get_token($pdo, $company_id);
    if (!$token) return [];

    $users = dingtalk_get_users($token, dingtalk_get_config($pdo, $company_id)['root_dept_id']);
    if (empty($users)) return [];

    $today = date('Y-m-d');
    $tomorrow = date('Y-m-d', strtotime('+1 day'));
    $result = [];

    $userids = array_keys($users);
    foreach (array_chunk($userids, 50) as $chunk) {
        $url = "https://oapi.dingtalk.com/attendance/listRecord?access_token=" . urlencode($token);
        $payload = json_encode([
            'userIds' => array_map('strval', $chunk),
            'checkDateFrom' => $today . ' 00:00:00',
            'checkDateTo' => $tomorrow . ' 00:00:00',
            'isI18n' => false
        ]);
        $ch = curl_init($url);
        curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
        curl_setopt($ch, CURLOPT_POST, true);
        curl_setopt($ch, CURLOPT_POSTFIELDS, $payload);
        curl_setopt($ch, CURLOPT_HTTPHEADER, ['Content-Type: application/json']);
        curl_setopt($ch, CURLOPT_TIMEOUT, 10);
        curl_setopt($ch, CURLOPT_SSL_VERIFYPEER, false);
        $resp = curl_exec($ch);
        curl_close($ch);

        $res = json_decode($resp, true);
        if (intval($res['errcode'] ?? -1) !== 0) continue;

        foreach (($res['recordresult'] ?? []) as $record) {
            $userid = (string)($record['userId'] ?? '');
            $time_result = (string)($record['timeResult'] ?? '');
            $user_check_time = intval($record['userCheckTime'] ?? 0);

            if ($userid !== '' && isset($users[$userid]) && $time_result !== 'NotSigned' && $user_check_time > 0) {
                $name = $users[$userid]['name'];
                $result[$name] = true;
            }
        }
    }

    return $result;
}
