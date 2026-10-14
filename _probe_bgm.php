<?php
// 临时验证：BGM 列 + 配置读写往返（用完即删）
require_once '/www/wwwroot/www.yacm.xin/tk/config.php';
require_once '/www/wwwroot/www.yacm.xin/tk/ai_key_lib.php';
require_once '/www/wwwroot/www.yacm.xin/tk/ai_rewrite_lib.php';

// 1) ensure_tables 幂等建列
ai_rw_ensure_tables($pdo);

// 2) 列存在检查
foreach (['bgm_base_url','bgm_api_key','bgm_model'] as $c) {
  $has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_config LIKE '$c'")->fetchAll();
  echo "config.$c: ", empty($has) ? 'MISSING' : 'ok', "\n";
}
$has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_tasks LIKE 'bgm_ai_result'")->fetchAll();
echo "tasks.bgm_ai_result: ", empty($has) ? 'MISSING' : 'ok', "\n";

// 3) 配置写入 + 读取往返（company 1）
ai_rw_save_config($pdo, 1, ['bgm_base_url' => 'https://api.siliconflow.cn/v1', 'bgm_api_key' => 'sk-test-roundtrip-123', 'bgm_model' => 'Qwen/Qwen3-Omni-30B-A3B-Instruct'], 0);
$cfg = ai_rw_get_config($pdo, 1);
echo "readback: base={$cfg['bgm_base_url']} key=", (strpos($cfg['bgm_api_key'], 'sk-test-roundtrip-123') === 0 ? 'decrypted-ok' : 'FAIL'), " model={$cfg['bgm_model']}\n";

// 4) resolve_config（credentials 用）也带 bgm
$rc = ai_key_resolve_config($pdo, 1);
echo "resolve: base={$rc['bgm_base_url']} key=", (strpos($rc['bgm_api_key'], 'sk-test-roundtrip-123') === 0 ? 'decrypted-ok' : 'FAIL'), " model={$rc['bgm_model']}\n";

// 5) 清理测试 key（保持空 = 未配置语义）
ai_rw_save_config($pdo, 1, ['bgm_api_key' => ''], 0);
echo "cleaned\n";
