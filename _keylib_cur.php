<?php
/**
 * ai_key_lib.php — AI 密钥保管库
 *
 * 设计：
 *   1. 统一密钥（company_id=0）为默认来源，公司可覆盖（company_id>0）
 *   2. 落库前 AES-256-CBC 加密，读取时解密；历史明文自动兼容（无 enc:v1: 前缀即视为明文）
 *   3. 加密密钥来自站点本地文件 ai_key_secret.php；文件不可写时用站点常量派生兜底
 *   4. 客户端令牌表 ai_client_tokens 供本地程序鉴权
 *
 * 依赖：config.php 已加载（用到 DB_NAME/DB_USER/DB_PASS 与 $pdo）
 * 兼容：PHP 7.4
 */

// ============================================================
// 加密
// ============================================================

if (!function_exists('ai_key_secret')) {
    /**
     * 站点加密密钥（32 字节 hex）。首次调用自动生成并落盘。
     */
    function ai_key_secret() {
        static $cached = null;
        if ($cached !== null) { return $cached; }

        $file = __DIR__ . '/ai_key_secret.php';
        if (is_file($file)) {
            $v = @include $file;
            if (is_string($v) && strlen($v) >= 32) { $cached = $v; return $cached; }
        }

        $new = bin2hex(random_bytes(32));
        $php = "<?php\n// 自动生成 · AI 密钥保管库加密密钥 · 请勿删除或外泄\nreturn '" . $new . "';\n";
        $ok = @file_put_contents($file, $php, LOCK_EX);
        if ($ok === false) {
            // 兜底：无法落盘时用站点常量派生，保证跨请求稳定（仍随数据库口令变化）
            $cached = hash('sha256', DB_NAME . '|' . DB_USER . '|' . DB_PASS . '|ai_key_vault_v1');
            return $cached;
        }
        @chmod($file, 0600);
        $cached = $new;
        return $cached;
    }
}

if (!function_exists('ai_key_is_encrypted')) {
    function ai_key_is_encrypted($v) {
        return strpos((string)$v, 'enc:v1:') === 0;
    }
}

if (!function_exists('ai_key_encrypt')) {
    /**
     * 加密单值。空值原样返回；已加密值幂等返回。
     * 失败时原样返回，绝不丢数据。
     */
    function ai_key_encrypt($plain) {
        $plain = (string)$plain;
        if ($plain === '') { return ''; }
        if (ai_key_is_encrypted($plain)) { return $plain; }

        $key = hash('sha256', ai_key_secret(), true);
        $iv  = random_bytes(16);
        $ct  = openssl_encrypt($plain, 'AES-256-CBC', $key, OPENSSL_RAW_DATA, $iv);
        if ($ct === false) { return $plain; }
        $mac = hash_hmac('sha256', $iv . $ct, $key, true);
        return 'enc:v1:' . base64_encode($iv . $mac . $ct);
    }
}

if (!function_exists('ai_key_decrypt')) {
    /**
     * 解密单值。历史明文（无前缀）原样返回；校验失败返回空串。
     */
    function ai_key_decrypt($cipher) {
        $cipher = (string)$cipher;
        if ($cipher === '') { return ''; }
        if (!ai_key_is_encrypted($cipher)) { return $cipher; }

        $raw = base64_decode(substr($cipher, 7), true);
        if ($raw === false || strlen($raw) < 48) { return ''; }
        $key = hash('sha256', ai_key_secret(), true);
        $iv  = substr($raw, 0, 16);
        $mac = substr($raw, 16, 32);
        $ct  = substr($raw, 48);
        if (!hash_equals($mac, hash_hmac('sha256', $iv . $ct, $key, true))) { return ''; }

        $pt = openssl_decrypt($ct, 'AES-256-CBC', $key, OPENSSL_RAW_DATA, $iv);
        return ($pt === false) ? '' : $pt;
    }
}

if (!function_exists('ai_key_sensitive_fields')) {
    /** 需要加密落库的字段 */
    function ai_key_sensitive_fields() {
        return ['asr_api_key', 'llm_api_key', 'translate_api_key', 'asr_proxy_pass', 'llm_proxy_pass', 'rewrite_llm_api_key', 'rewrite_llm_proxy_pass'];
    }
}

if (!function_exists('ai_key_encrypt_fields')) {
    function ai_key_encrypt_fields($row) {
        foreach (ai_key_sensitive_fields() as $f) {
            if (isset($row[$f]) && (string)$row[$f] !== '') { $row[$f] = ai_key_encrypt($row[$f]); }
        }
        return $row;
    }
}

if (!function_exists('ai_key_decrypt_fields')) {
    function ai_key_decrypt_fields($row) {
        foreach (ai_key_sensitive_fields() as $f) {
            if (isset($row[$f]) && (string)$row[$f] !== '') { $row[$f] = ai_key_decrypt($row[$f]); }
        }
        return $row;
    }
}

// ============================================================
// 配置读取（统一价 / 公司覆盖）
// ============================================================

if (!function_exists('ai_key_raw_config_row')) {
    /** 读取原始配置行（未解密、未填默认），无则 null */
    function ai_key_raw_config_row($pdo, $company_id) {
        try {
            if (function_exists('ai_bill_ensure_tables')) { ai_bill_ensure_tables($pdo); }
            if (function_exists('ai_rw_ensure_tables'))   { ai_rw_ensure_tables($pdo); }
            $st = $pdo->prepare("SELECT * FROM `ai_rewrite_config` WHERE company_id = ? LIMIT 1");
            $st->execute([(int)$company_id]);
            $row = $st->fetch(PDO::FETCH_ASSOC);
            return $row ? $row : null;
        } catch (Exception $e) {
            return null;
        }
    }
}

if (!function_exists('ai_key_resolve_config')) {
    /**
     * 生效配置 = 统一配置(company_id=0) → 公司覆盖(company_id>0) → 默认提示词。
     * 返回字段已解密，可直接用于调用。
     */
    function ai_key_resolve_config($pdo, $company_id) {
        $company_id = (int)$company_id;

        $fields = [
            'asr_provider', 'asr_base_url', 'asr_api_key', 'asr_model',
            'llm_provider', 'llm_base_url', 'llm_api_key', 'llm_model',
            'rewrite_llm_base_url', 'rewrite_llm_api_key', 'rewrite_llm_model',
            'rewrite_llm_proxy', 'rewrite_llm_proxy_type', 'rewrite_llm_proxy_user', 'rewrite_llm_proxy_pass',
            'analyze_prompt', 'rewrite_prompt', 'score_prompt',
            'translate_model', 'translate_base_url', 'translate_api_key', 'translate_prompt',
            'asr_proxy', 'asr_proxy_type', 'asr_proxy_user', 'asr_proxy_pass',
            'llm_proxy', 'llm_proxy_type', 'llm_proxy_user', 'llm_proxy_pass',
        ];

        $global  = ai_key_raw_config_row($pdo, 0);
        $company = ($company_id > 0) ? ai_key_raw_config_row($pdo, $company_id) : null;

        $out = [];
        foreach ($fields as $f) {
            $v = '';
            if ($global  && isset($global[$f])  && trim((string)$global[$f])  !== '') { $v = (string)$global[$f]; }
            if ($company && isset($company[$f]) && trim((string)$company[$f]) !== '') { $v = (string)$company[$f]; }
            $out[$f] = $v;
        }

        // 接口地址兜底
        if ($out['asr_base_url'] === '') { $out['asr_base_url'] = 'https://api.siliconflow.cn/v1'; }
        if ($out['asr_model'] === '')    { $out['asr_model'] = 'FunAudioLLM/SenseVoiceSmall'; }
        if ($out['llm_base_url'] === '') { $out['llm_base_url'] = 'https://api.deepseek.com/v1'; }
        if ($out['llm_model'] === '')    { $out['llm_model'] = 'deepseek-chat'; }
        if ($out['asr_provider'] === '') { $out['asr_provider'] = 'openai_compatible'; }
        if ($out['llm_provider'] === '') { $out['llm_provider'] = 'openai_compatible'; }
        if ($out['asr_proxy_type'] === '') { $out['asr_proxy_type'] = 'socks5h'; }
        if ($out['llm_proxy_type'] === '') { $out['llm_proxy_type'] = 'socks5h'; }
        if ($out['rewrite_llm_proxy_type'] === '') { $out['rewrite_llm_proxy_type'] = 'socks5h'; }

        // 提示词兜底（共享知识库）
        $defaults = function_exists('ai_rw_default_prompts')
            ? ai_rw_default_prompts()
            : ['analyze' => '', 'rewrite' => '', 'score' => ''];
        if ($out['analyze_prompt'] === '') { $out['analyze_prompt'] = $defaults['analyze']; }
        if ($out['rewrite_prompt'] === '') { $out['rewrite_prompt'] = $defaults['rewrite']; }
        if ($out['score_prompt']   === '') { $out['score_prompt']   = $defaults['score']; }

        // 翻译兜底：模型用免费翻译模型，接口地址/Key 默认复用 ASR（同属硅基流动）
        if ($out['translate_model'] === '') {
            $out['translate_model'] = function_exists('ai_rw_default_translate_model')
                ? ai_rw_default_translate_model() : 'tencent/Hunyuan-MT-7B';
        }
        if ($out['translate_base_url'] === '') { $out['translate_base_url'] = $out['asr_base_url']; }
        if ($out['translate_api_key'] === '')  { $out['translate_api_key']  = $out['asr_api_key']; }
        if ($out['translate_prompt'] === '' && isset($defaults['translate'])) { $out['translate_prompt'] = $defaults['translate']; }

        $out = ai_key_decrypt_fields($out);
        $out['company_id'] = $company_id;
        return $out;
    }
}

if (!function_exists('ai_key_config_ready')) {
    function ai_key_config_ready($cfg) {
        return trim((string)$cfg['asr_api_key']) !== '' && trim((string)$cfg['llm_api_key']) !== '';
    }
}

// ============================================================
// 客户端令牌
// ============================================================

if (!function_exists('ai_key_ensure_client_tables')) {
    function ai_key_ensure_client_tables($pdo) {
        static $done = false;
        if ($done) { return; }
        $done = true;
        try {
            $pdo->exec("CREATE TABLE IF NOT EXISTS `ai_client_tokens` (
                `id` int(11) NOT NULL AUTO_INCREMENT,
                `user_id` int(11) NOT NULL DEFAULT 0,
                `company_id` int(11) NOT NULL DEFAULT 0,
                `token` varchar(64) NOT NULL,
                `device` varchar(128) NOT NULL DEFAULT '',
                `expires_at` datetime NOT NULL,
                `created_at` datetime DEFAULT CURRENT_TIMESTAMP,
                `last_used_at` datetime DEFAULT NULL,
                PRIMARY KEY (`id`),
                UNIQUE KEY `uk_token` (`token`),
                KEY `idx_user` (`user_id`),
                KEY `idx_expires` (`expires_at`)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='AI洗稿-本地程序令牌'");
        } catch (Exception $e) {}
    }
}