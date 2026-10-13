<?php
/**
 * ai_rewrite_lib.php — 爆文库 AI 洗稿核心库
 *
 * 职责：
 *   1. 建表（幂等）：ai_rewrite_config / ai_rewrite_tasks / viral_points / ai_scripts
 *   2. AI 配置读写（按公司隔离）
 *   3. 媒体下载 → ASR 转写 → 爆点分析 → 洗稿，四步流水线
 *   4. 爆款核心沉淀入库，供后续 AI 再次读取分析
 *
 * 依赖：仅 curl + fileinfo + json（服务器已确认存在），不依赖 ffmpeg。
 * 兼容：PHP 7.4（不使用 PHP 8 专有函数）。
 */

require_once __DIR__ . '/ai_key_lib.php';
require_once __DIR__ . '/ai_billing_lib.php';

// ============================================================
// 建表
// ============================================================

function ai_rw_ensure_tables($pdo) {
    static $done = false;
    if ($done) { return; }

    $pdo->exec("CREATE TABLE IF NOT EXISTS ai_rewrite_config (
        id INT AUTO_INCREMENT PRIMARY KEY,
        company_id INT NOT NULL DEFAULT 0,
        asr_provider VARCHAR(30) NOT NULL DEFAULT 'openai_compatible',
        asr_base_url VARCHAR(255) NOT NULL DEFAULT '',
        asr_api_key VARCHAR(500) NOT NULL DEFAULT '',
        asr_model VARCHAR(100) NOT NULL DEFAULT '',
        llm_provider VARCHAR(30) NOT NULL DEFAULT 'openai_compatible',
        llm_base_url VARCHAR(255) NOT NULL DEFAULT '',
        llm_api_key VARCHAR(500) NOT NULL DEFAULT '',
        llm_model VARCHAR(100) NOT NULL DEFAULT '',
        analyze_prompt TEXT,
        rewrite_prompt TEXT,
        score_prompt TEXT,
        translate_model VARCHAR(100) NOT NULL DEFAULT '',
        translate_base_url VARCHAR(255) NOT NULL DEFAULT '',
        translate_api_key VARCHAR(500) NOT NULL DEFAULT '',
        translate_prompt TEXT,
        asr_proxy VARCHAR(255) NOT NULL DEFAULT '',
        asr_proxy_type VARCHAR(20) NOT NULL DEFAULT 'http',
        asr_proxy_user VARCHAR(100) NOT NULL DEFAULT '',
        asr_proxy_pass VARCHAR(200) NOT NULL DEFAULT '',
        llm_proxy VARCHAR(255) NOT NULL DEFAULT '',
        llm_proxy_type VARCHAR(20) NOT NULL DEFAULT 'http',
        llm_proxy_user VARCHAR(100) NOT NULL DEFAULT '',
        llm_proxy_pass VARCHAR(200) NOT NULL DEFAULT '',
        updated_by INT NOT NULL DEFAULT 0,
        created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
        updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
        UNIQUE KEY uk_company (company_id)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='AI洗稿-配置'");

    $pdo->exec("CREATE TABLE IF NOT EXISTS ai_rewrite_tasks (
        id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
        company_id INT NOT NULL DEFAULT 0,
        user_id INT NOT NULL DEFAULT 0,
        source VARCHAR(20) NOT NULL DEFAULT 'baowenku',
        video_id VARCHAR(100) NOT NULL DEFAULT '',
        account_id INT NOT NULL DEFAULT 0,
        account_type VARCHAR(20) NOT NULL DEFAULT '',
        platform VARCHAR(20) NOT NULL DEFAULT '',
        category VARCHAR(50) NOT NULL DEFAULT '',
        title VARCHAR(255) NOT NULL DEFAULT '',
        source_url VARCHAR(600) NOT NULL DEFAULT '',
        source_key VARCHAR(191) NOT NULL DEFAULT '',
        cover_url VARCHAR(600) NOT NULL DEFAULT '',
        play_count BIGINT NOT NULL DEFAULT 0,
        digg_count BIGINT NOT NULL DEFAULT 0,
        comment_count BIGINT NOT NULL DEFAULT 0,
        video_duration INT NOT NULL DEFAULT 0,
        asr_text MEDIUMTEXT,
        asr_text_zh MEDIUMTEXT,
        asr_status TINYINT NOT NULL DEFAULT 0,
        asr_manual TINYINT NOT NULL DEFAULT 0,
        analyze_json LONGTEXT,
        analyze_status TINYINT NOT NULL DEFAULT 0,
        rewrite_text MEDIUMTEXT,
        rewrite_text_zh MEDIUMTEXT,
        rewrite_json LONGTEXT,
        rewrite_status TINYINT NOT NULL DEFAULT 0,
        retry_count TINYINT NOT NULL DEFAULT 0,
        reused_from BIGINT UNSIGNED NOT NULL DEFAULT 0,
        error_msg VARCHAR(500) NOT NULL DEFAULT '',
        created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
        updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
        PRIMARY KEY (id),
        UNIQUE KEY uk_video (company_id, video_id),
        KEY idx_source_key (company_id, source_key),
        KEY idx_asr (asr_status),
        KEY idx_analyze (asr_status, analyze_status),
        KEY idx_rewrite (analyze_status, rewrite_status),
        KEY idx_company_created (company_id, created_at)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='AI洗稿-任务'");

    $pdo->exec("CREATE TABLE IF NOT EXISTS viral_points (
        id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
        company_id INT NOT NULL DEFAULT 0,
        task_id BIGINT UNSIGNED NOT NULL DEFAULT 0,
        video_id VARCHAR(100) NOT NULL DEFAULT '',
        platform VARCHAR(20) NOT NULL DEFAULT '',
        category VARCHAR(50) NOT NULL DEFAULT '',
        work VARCHAR(255) NOT NULL DEFAULT '',
        title VARCHAR(255) NOT NULL DEFAULT '',
        source_url VARCHAR(600) NOT NULL DEFAULT '',
        play_count BIGINT NOT NULL DEFAULT 0,
        hook VARCHAR(500) NOT NULL DEFAULT '',
        structure_json TEXT,
        points_json TEXT,
        emotion VARCHAR(100) NOT NULL DEFAULT '',
        keywords VARCHAR(500) NOT NULL DEFAULT '',
        reusable VARCHAR(500) NOT NULL DEFAULT '',
        summary TEXT,
        asr_text MEDIUMTEXT,
        created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
        updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
        PRIMARY KEY (id),
        UNIQUE KEY uk_task (task_id),
        KEY idx_company_cat (company_id, category),
        KEY idx_company_work (company_id, work),
        KEY idx_play (play_count)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='AI洗稿-爆款核心库'");

    $pdo->exec("CREATE TABLE IF NOT EXISTS ai_scripts (
        id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
        company_id INT NOT NULL DEFAULT 0,
        user_id INT NOT NULL DEFAULT 0,
        task_id BIGINT UNSIGNED NOT NULL DEFAULT 0,
        point_id BIGINT UNSIGNED NOT NULL DEFAULT 0,
        category VARCHAR(50) NOT NULL DEFAULT '',
        title VARCHAR(255) NOT NULL DEFAULT '',
        hook VARCHAR(500) NOT NULL DEFAULT '',
        body TEXT,
        cta VARCHAR(500) NOT NULL DEFAULT '',
        tags VARCHAR(500) NOT NULL DEFAULT '',
        kept_points TEXT,
        full_text MEDIUMTEXT,
        status TINYINT NOT NULL DEFAULT 0,
        score TINYINT DEFAULT NULL,
        score_json LONGTEXT,
        scored_at DATETIME DEFAULT NULL,
        created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
        updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
        PRIMARY KEY (id),
        KEY idx_task (task_id),
        KEY idx_company_cat (company_id, category),
        KEY idx_status (company_id, status),
        KEY idx_score (company_id, score)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='AI洗稿-文案库'");

    // 老表补列（幂等）：source_key / reused_from
    $has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_tasks LIKE 'source_key'")->fetchAll();
    if (empty($has)) {
        $pdo->exec("ALTER TABLE ai_rewrite_tasks ADD COLUMN source_key VARCHAR(191) NOT NULL DEFAULT '' AFTER source_url");
        $pdo->exec("ALTER TABLE ai_rewrite_tasks ADD INDEX idx_source_key (company_id, source_key)");
    }
    $has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_tasks LIKE 'reused_from'")->fetchAll();
    if (empty($has)) {
        $pdo->exec("ALTER TABLE ai_rewrite_tasks ADD COLUMN reused_from BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER retry_count");
    }

    // 老表补列（幂等）：打分字段
    $has = $pdo->query("SHOW COLUMNS FROM ai_scripts LIKE 'score'")->fetchAll();
    if (empty($has)) {
        $pdo->exec("ALTER TABLE ai_scripts ADD COLUMN score TINYINT DEFAULT NULL AFTER status");
        $pdo->exec("ALTER TABLE ai_scripts ADD COLUMN score_json LONGTEXT AFTER score");
        $pdo->exec("ALTER TABLE ai_scripts ADD COLUMN scored_at DATETIME DEFAULT NULL AFTER score_json");
        $pdo->exec("ALTER TABLE ai_scripts ADD INDEX idx_score (company_id, score)");
    }

    // 老表补列（幂等）：打分提示词
    $has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_config LIKE 'score_prompt'")->fetchAll();
    if (empty($has)) {
        $pdo->exec("ALTER TABLE ai_rewrite_config ADD COLUMN score_prompt TEXT AFTER rewrite_prompt");
    }

    // 老表补列（幂等）：手动转写标记
    $has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_tasks LIKE 'asr_manual'")->fetchAll();
    if (empty($has)) {
        $pdo->exec("ALTER TABLE ai_rewrite_tasks ADD COLUMN asr_manual TINYINT NOT NULL DEFAULT 0 AFTER asr_status");
    }

    // 老表补列（幂等）：原文/洗稿的中文译文（供人工对照核查）
    $has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_tasks LIKE 'asr_text_zh'")->fetchAll();
    if (empty($has)) {
        $pdo->exec("ALTER TABLE ai_rewrite_tasks ADD COLUMN asr_text_zh MEDIUMTEXT AFTER asr_text");
    }
    $has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_tasks LIKE 'rewrite_text_zh'")->fetchAll();
    if (empty($has)) {
        $pdo->exec("ALTER TABLE ai_rewrite_tasks ADD COLUMN rewrite_text_zh MEDIUMTEXT AFTER rewrite_text");
    }

    // 老表补列（幂等）：翻译配置
    $has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_config LIKE 'translate_model'")->fetchAll();
    if (empty($has)) {
        $pdo->exec("ALTER TABLE ai_rewrite_config ADD COLUMN translate_model VARCHAR(100) NOT NULL DEFAULT '' AFTER score_prompt");
        $pdo->exec("ALTER TABLE ai_rewrite_config ADD COLUMN translate_base_url VARCHAR(255) NOT NULL DEFAULT '' AFTER translate_model");
        $pdo->exec("ALTER TABLE ai_rewrite_config ADD COLUMN translate_api_key VARCHAR(500) NOT NULL DEFAULT '' AFTER translate_base_url");
        $pdo->exec("ALTER TABLE ai_rewrite_config ADD COLUMN translate_prompt TEXT AFTER translate_api_key");
    }

    // 老表补列（幂等）：爆款核心库的作品标签
    $has = $pdo->query("SHOW COLUMNS FROM viral_points LIKE 'work'")->fetchAll();
    if (empty($has)) {
        $pdo->exec("ALTER TABLE viral_points ADD COLUMN work VARCHAR(255) NOT NULL DEFAULT '' AFTER category");
        $pdo->exec("ALTER TABLE viral_points ADD INDEX idx_company_work (company_id, work)");
    }

    // 老表补列（幂等）：ASR / LLM 出站代理（Claude 等官方 API 必须走指定线路，换 IP 会封号）
    $proxy_cols = [
        'asr_proxy' => "VARCHAR(255) NOT NULL DEFAULT ''",
        'asr_proxy_type' => "VARCHAR(20) NOT NULL DEFAULT 'http'",
        'asr_proxy_user' => "VARCHAR(100) NOT NULL DEFAULT ''",
        'asr_proxy_pass' => "VARCHAR(200) NOT NULL DEFAULT ''",
        'llm_proxy' => "VARCHAR(255) NOT NULL DEFAULT ''",
        'llm_proxy_type' => "VARCHAR(20) NOT NULL DEFAULT 'http'",
        'llm_proxy_user' => "VARCHAR(100) NOT NULL DEFAULT ''",
        'llm_proxy_pass' => "VARCHAR(200) NOT NULL DEFAULT ''",
    ];
    foreach ($proxy_cols as $col => $def) {
        $has = $pdo->query("SHOW COLUMNS FROM ai_rewrite_config LIKE '{$col}'")->fetchAll();
        if (empty($has)) {
            $pdo->exec("ALTER TABLE ai_rewrite_config ADD COLUMN {$col} {$def}");
        }
    }

    $done = true;
}

// ============================================================
// 链接归一化 · 去重 · 结果复用
// ============================================================

/**
 * 把视频链接归一化成"同一个视频"的稳定标识。
 * 去掉 #片段、小写化域名、去掉末尾斜杠，并剔除易变的签名/过期参数，
 * 使同一视频即使签名参数变化也能识别为同一条。
 */
function ai_rw_norm_url($url) {
    $url = trim((string)$url);
    if ($url === '') { return ''; }
    $url = preg_replace('/#.*$/', '', $url);
    $p = @parse_url($url);
    if (!$p || empty($p['host'])) { return mb_substr($url, 0, 191); }

    $scheme = strtolower(isset($p['scheme']) ? $p['scheme'] : 'http');
    $host   = strtolower($p['host']);
    $path   = isset($p['path']) ? rtrim($p['path'], '/') : '';

    $query = '';
    if (!empty($p['query'])) {
        parse_str($p['query'], $q);
        // 只剔除明显易变的签名/过期/埋点参数，保留 id / v 等身份参数
        $volatile = ['x-expires','x-signature','expire','sig','n','refresh_token','dr','t','bt','ft','l','rc',
            'btag','vvpl','o','dy_q','lk3s','qs','lr','cd','cv','br','cs','ds','cr','ch','from','se',
            'mh','mm','mn','ms','mt','mv','mvi','pl','initcwndbps','ratebypass','requiressl','source',
            'sver','txp','sparams','c','ei','ip','idc','biz_tag','ps','shp','shcp','s','sc','mime_type',
            'bti','a','feature_id','q','b','mime','clen','dur','key','cpn','pcm2','rbuf','spc','ump',
            'gir','host','_','timestamp','ts'];
        foreach ($volatile as $k) { unset($q[$k]); }
        if (!empty($q)) { ksort($q); $query = '?' . http_build_query($q); }
    }
    return mb_substr($scheme . '://' . $host . $path . $query, 0, 191);
}

/**
 * 为"手动输入的链接"推导一个稳定的 video_id，用于同链接去重。
 * 优先抽取平台视频 ID（YouTube v=/shorts/、抖音/TikTok /video/xxx），抽不到再用归一化链接哈希。
 */
function ai_rw_derive_video_id($url) {
    $url = trim((string)$url);
    $id = '';
    if (preg_match('#[?&]v=([A-Za-z0-9_\-]{6,})#', $url, $m)) { $id = $m[1]; }
    elseif (preg_match('#/(?:shorts|embed|watch)/([A-Za-z0-9_\-]{6,})#', $url, $m)) { $id = $m[1]; }
    elseif (preg_match('#/video/(\d{6,})#', $url, $m)) { $id = $m[1]; }
    if ($id !== '') { return 'url_' . substr(md5($id), 0, 20); }
    $norm = ai_rw_norm_url($url);
    return 'url_' . substr(md5($norm !== '' ? $norm : $url), 0, 20);
}

/**
 * 由「平台 + 视频ID + 账号 handle」拼出稳定的视频页面链接。
 * 不使用 CDN 直链（play_url）——它是带签名的地址、有时效，过期后无法下载。
 */
function ai_rw_build_video_url($platform, $video_id, $handle = '') {
    $platform = strtolower(trim((string)$platform));
    $video_id = trim((string)$video_id);
    if ($video_id === '') { return ''; }
    if ($platform === 'youtube') { return 'https://www.youtube.com/watch?v=' . $video_id; }
    if ($platform === 'douyin')  { return 'https://www.douyin.com/video/' . $video_id; }
    $handle = ltrim(trim((string)$handle), '@');
    if ($handle === '') { return ''; }
    return 'https://www.tiktok.com/@' . $handle . '/video/' . $video_id;
}

/**
 * 查找同公司、同链接且已有转写结果的历史任务，用于复用。
 */
function ai_rw_find_cached_task($pdo, $company_id, $source_key, $exclude_id = 0) {
    $source_key = trim((string)$source_key);
    if ($source_key === '') { return null; }
    $sql = "SELECT * FROM ai_rewrite_tasks
            WHERE company_id = ? AND source_key = ? AND asr_text IS NOT NULL AND asr_text <> ''";
    $params = [(int)$company_id, $source_key];
    if ((int)$exclude_id > 0) { $sql .= " AND id <> ?"; $params[] = (int)$exclude_id; }
    $sql .= " ORDER BY (rewrite_status = 2) DESC, (analyze_status = 2) DESC, id DESC LIMIT 1";
    $st = $pdo->prepare($sql);
    $st->execute($params);
    $row = $st->fetch(PDO::FETCH_ASSOC);
    return $row ? $row : null;
}

/**
 * 统一建任务入口：自动去重 + 复用历史结果（不重复调用 API）。
 *
 * $fields 需含：source, video_id, source_url；可选 account_id/account_type/platform/
 *              category/title/cover_url/play_count/digg_count/comment_count/video_duration
 *
 * 返回：['status' => created|reused|skipped, 'task_id' => int, 'reused_from' => int]
 */
function ai_rw_create_task($pdo, $company_id, $user_id, $fields) {
    ai_rw_ensure_tables($pdo);
    $company_id = (int)$company_id;
    $user_id    = (int)$user_id;

    $source_url = trim((string)(isset($fields['source_url']) ? $fields['source_url'] : ''));
    $video_id   = trim((string)(isset($fields['video_id']) ? $fields['video_id'] : ''));
    if ($source_url === '') { return ['status' => 'error', 'message' => '缺少视频链接']; }
    if ($video_id === '')   { $video_id = ai_rw_derive_video_id($source_url); }

    $source_key = ai_rw_norm_url($source_url);

    // 1) 同一 video_id 已存在 → 直接跳过（不重复建、不重复扣费）
    $chk = $pdo->prepare("SELECT id FROM ai_rewrite_tasks WHERE company_id = ? AND video_id = ? LIMIT 1");
    $chk->execute([$company_id, $video_id]);
    $exist = (int)$chk->fetchColumn();
    if ($exist > 0) {
        return ['status' => 'skipped', 'task_id' => $exist];
    }

    // 2) 同链接的历史结果 → 复用
    $cache = ai_rw_find_cached_task($pdo, $company_id, $source_key);

    $data = [
        'company_id'     => $company_id,
        'user_id'        => $user_id,
        'source'         => (string)(isset($fields['source']) ? $fields['source'] : 'baowenku'),
        'video_id'       => $video_id,
        'account_id'     => (int)(isset($fields['account_id']) ? $fields['account_id'] : 0),
        'account_type'   => (string)(isset($fields['account_type']) ? $fields['account_type'] : ''),
        'platform'       => (string)(isset($fields['platform']) ? $fields['platform'] : ''),
        'category'       => (string)(isset($fields['category']) ? $fields['category'] : ''),
        'title'          => mb_substr((string)(isset($fields['title']) ? $fields['title'] : ''), 0, 255),
        'source_url'     => $source_url,
        'source_key'     => $source_key,
        'cover_url'      => (string)(isset($fields['cover_url']) ? $fields['cover_url'] : ''),
        'play_count'     => (int)(isset($fields['play_count']) ? $fields['play_count'] : 0),
        'digg_count'     => (int)(isset($fields['digg_count']) ? $fields['digg_count'] : 0),
        'comment_count'  => (int)(isset($fields['comment_count']) ? $fields['comment_count'] : 0),
        'video_duration' => (int)(isset($fields['video_duration']) ? $fields['video_duration'] : 0),
        'asr_status'     => 0,
        'analyze_status' => 0,
        'rewrite_status' => 0,
    ];

    $reused = false;
    if ($cache) {
        $reused = true;
        $data['reused_from'] = (int)$cache['id'];
        if (!empty($cache['asr_text'])) {
            $data['asr_text']   = $cache['asr_text'];
            $data['asr_status'] = 2;
        }
        if ((int)$cache['analyze_status'] === 2 && !empty($cache['analyze_json'])) {
            $data['analyze_json']   = $cache['analyze_json'];
            $data['analyze_status'] = 2;
            if ($data['category'] === '' && !empty($cache['category'])) { $data['category'] = $cache['category']; }
        }
        if ((int)$cache['rewrite_status'] === 2 && !empty($cache['rewrite_text'])) {
            $data['rewrite_json']   = $cache['rewrite_json'];
            $data['rewrite_text']   = $cache['rewrite_text'];
            $data['rewrite_status'] = 2;
        }
    }

    $cols = array_keys($data);
    $ph = array_fill(0, count($cols), '?');
    $pdo->prepare("INSERT INTO ai_rewrite_tasks (`" . implode('`,`', $cols) . "`) VALUES (" . implode(',', $ph) . ")")
        ->execute(array_values($data));

    return [
        'status'      => $reused ? 'reused' : 'created',
        'task_id'     => (int)$pdo->lastInsertId(),
        'reused_from' => $cache ? (int)$cache['id'] : 0,
    ];
}

// ============================================================
// 配置
// ============================================================

/**
 * 默认提示词。动漫/影视解说赛道特化。
 */
function ai_rw_default_prompts() {
    return [
        'analyze' => "你是短视频爆款分析师，专精【动漫解说】【影视解说】赛道。\n"
            . "请分析下面这条爆款视频的口播文案，提炼它为什么能爆。\n\n"
            . "只输出 JSON，不要任何解释，结构如下：\n"
            . "{\n"
            . "  \"category\": \"动漫解说 或 影视解说 或 其他\",\n"
            . "  \"work\": \"作品规范名：认出这是哪部动漫/电影/剧集时输出中文常用名（如：海贼王、流浪地球），认不出输出空字符串\",\n"
            . "  \"work_aliases\": [\"同一作品的其他常见叫法，如：航海王、One Piece，没有则空数组\"],\n"
            . "  \"hook\": \"开头3秒钩子的原文，以及它为什么能留住人\",\n"
            . "  \"structure\": [{\"stage\":\"开头/铺垫/反转/高潮/收尾\",\"desc\":\"这一段做了什么\"}],\n"
            . "  \"points\": [\"爆点1：悬念/反转/情绪/信息差/爽点，具体说明\"],\n"
            . "  \"emotion\": \"主导情绪（如愤怒、意难平、爽、好奇、共鸣）\",\n"
            . "  \"keywords\": [\"关键词1\",\"关键词2\"],\n"
            . "  \"reusable\": \"这条视频可复用的套路公式（一句话）\",\n"
            . "  \"summary\": \"一句话概括它的爆款逻辑\"\n"
            . "}",

        'rewrite' => "你是【动漫解说】【影视解说】的爆款口播文案写手。\n"
            . "请依据【爆款核心】与【原口播稿】，写一条全新的原创解说文案。\n\n"
            . "硬性要求：\n"
            . "1. 保留爆点内核（悬念、反转、情绪、信息差、爽点），但表达方式必须全部替换。\n"
            . "2. 严禁逐句对照改写；严禁照搬原片名+人物名的组合叙述顺序，要换角度切入。\n"
            . "3. 前3秒必须有强钩子，能让人停下来。\n"
            . "4. 全篇口语化，适合直接朗读，时长控制在 30-60 秒。\n"
            . "5. 不得出现原视频的账号名、水印、外链。\n"
            . "6. 输出语言必须与【原口播稿】完全一致：原文是英语就用英语写，原文是中文就用中文写，\n"
            . "   原文是日语就用日语写；严禁翻译成其他语言。\n"
            . "7. 只输出 JSON，不要任何解释。\n\n"
            . "输出结构：\n"
            . "{\n"
            . "  \"title\": \"标题\",\n"
            . "  \"hook\": \"前3秒钩子\",\n"
            . "  \"body\": [\"正文第1段\",\"正文第2段\",\"正文第3段\"],\n"
            . "  \"cta\": \"结尾引导\",\n"
            . "  \"tags\": [\"标签1\",\"标签2\"],\n"
            . "  \"kept_points\": [\"保留的爆点1\",\"保留的爆点2\"]\n"
            . "}",

        'score' => "你是短视频爆款文案的严格评审员，专精【动漫解说】【影视解说】赛道。\n"
            . "请对下面这条【洗稿后的解说文案】打分，满分 100 分，并给出可落地的改进建议。\n\n"
            . "评分维度（各占权重，自行综合成总分）：\n"
            . "1. 前3秒钩子强度（能否让人停下来）\n"
            . "2. 情绪张力与共鸣（愤怒/意难平/爽/好奇）\n"
            . "3. 原创度（是否明显照搬原片叙述顺序）\n"
            . "4. 节奏与口语化（是否顺口、适合朗读）\n"
            . "5. 爆点保留（悬念/反转/信息差是否到位）\n\n"
            . "打分标准：90-100 极强爆款潜质；75-89 良好；60-74 及格可优化；60 以下需重写。\n\n"
            . "只输出 JSON，不要任何解释，结构如下：\n"
            . "{\n"
            . "  \"score\": 0到100的整数,\n"
            . "  \"level\": \"极强/良好/及格/待改进\",\n"
            . "  \"dimensions\": [{\"name\":\"维度名\",\"score\":0到100,\"comment\":\"一句话点评\"}],\n"
            . "  \"suggestions\": [\"具体改进建议1\",\"具体改进建议2\",\"具体改进建议3\"],\n"
            . "  \"summary\": \"一句话总评\"\n"
            . "}",

        'translate' => "你是专业翻译。请把下面的短视频口播文案翻译成{目标语言}。\n"
            . "要求：\n"
            . "1. 只翻译，不改写、不增删内容、不总结。\n"
            . "2. 保持口语化，贴合短视频口播的语气与节奏。\n"
            . "3. 保留原文的段落与换行结构。\n"
            . "4. 专有名词（人名、片名、平台名）保留原文或通用译名，不要生造。\n"
            . "5. 只输出译文本身，不要任何解释、标题或引号。",
    ];
}

/** 默认翻译模型（硅基流动免费翻译模型）。 */
function ai_rw_default_translate_model() {
    return 'tencent/Hunyuan-MT-7B';
}

/**
 * 读取公司 AI 配置；未配置时返回默认值（api_key 为空）。
 */
function ai_rw_get_config($pdo, $company_id) {
    ai_rw_ensure_tables($pdo);
    $company_id = (int)$company_id;
    $stmt = $pdo->prepare("SELECT * FROM ai_rewrite_config WHERE company_id = ? LIMIT 1");
    $stmt->execute([$company_id]);
    $row = $stmt->fetch(PDO::FETCH_ASSOC);
    $defaults = ai_rw_default_prompts();

    if (!$row) {
        return [
            'company_id' => $company_id,
            'asr_provider' => 'openai_compatible',
            'asr_base_url' => 'https://api.siliconflow.cn/v1',
            'asr_api_key' => '',
            'asr_model' => 'FunAudioLLM/SenseVoiceSmall',
            'llm_provider' => 'openai_compatible',
            'llm_base_url' => 'https://api.deepseek.com/v1',
            'llm_api_key' => '',
            'llm_model' => 'deepseek-chat',
            'analyze_prompt' => $defaults['analyze'],
            'rewrite_prompt' => $defaults['rewrite'],
            'score_prompt'   => $defaults['score'],
            'translate_model' => ai_rw_default_translate_model(),
            'translate_base_url' => '',
            'translate_api_key' => '',
            'translate_prompt' => $defaults['translate'],
            'asr_proxy' => '',
            'asr_proxy_type' => 'http',
            'asr_proxy_user' => '',
            'asr_proxy_pass' => '',
            'llm_proxy' => '',
            'llm_proxy_type' => 'http',
            'llm_proxy_user' => '',
            'llm_proxy_pass' => '',
        ];
    }

    $row['analyze_prompt'] = trim((string)$row['analyze_prompt']) !== '' ? $row['analyze_prompt'] : $defaults['analyze'];
    $row['rewrite_prompt'] = trim((string)$row['rewrite_prompt']) !== '' ? $row['rewrite_prompt'] : $defaults['rewrite'];
    $row['score_prompt']   = (isset($row['score_prompt']) && trim((string)$row['score_prompt']) !== '') ? $row['score_prompt'] : $defaults['score'];
    $row['translate_prompt'] = (isset($row['translate_prompt']) && trim((string)$row['translate_prompt']) !== '') ? $row['translate_prompt'] : $defaults['translate'];
    if (trim((string)($row['translate_model'] ?? '')) === '') { $row['translate_model'] = ai_rw_default_translate_model(); }
    // 敏感字段解密（历史明文自动兼容）
    $row = ai_key_decrypt_fields($row);
    return $row;
}

function ai_rw_save_config($pdo, $company_id, $data, $user_id = 0) {
    ai_rw_ensure_tables($pdo);
    if (function_exists('ai_bill_ensure_tables')) { ai_bill_ensure_tables($pdo); }
    $company_id = (int)$company_id;
    $defaults = ai_rw_default_prompts();

    $fields = [
        'asr_provider', 'asr_base_url', 'asr_api_key', 'asr_model',
        'llm_provider', 'llm_base_url', 'llm_api_key', 'llm_model',
        'analyze_prompt', 'rewrite_prompt', 'score_prompt',
        'translate_model', 'translate_base_url', 'translate_api_key', 'translate_prompt',
        'asr_proxy', 'asr_proxy_type', 'asr_proxy_user', 'asr_proxy_pass',
        'llm_proxy', 'llm_proxy_type', 'llm_proxy_user', 'llm_proxy_pass',
    ];
    $vals = [];
    foreach ($fields as $f) {
        if (!array_key_exists($f, $data)) { continue; }
        $vals[$f] = (string)$data[$f];
    }
    if (isset($vals['analyze_prompt']) && trim($vals['analyze_prompt']) === '') { $vals['analyze_prompt'] = $defaults['analyze']; }
    if (isset($vals['rewrite_prompt']) && trim($vals['rewrite_prompt']) === '') { $vals['rewrite_prompt'] = $defaults['rewrite']; }
    if (isset($vals['score_prompt']) && trim($vals['score_prompt']) === '') { $vals['score_prompt'] = $defaults['score']; }
    if (isset($vals['translate_prompt']) && trim($vals['translate_prompt']) === '') { $vals['translate_prompt'] = $defaults['translate']; }
    if (isset($vals['translate_model']) && trim($vals['translate_model']) === '') { $vals['translate_model'] = ai_rw_default_translate_model(); }

    // 敏感字段加密落库（api_key / 代理口令）
    $vals = ai_key_encrypt_fields($vals);

    $stmt = $pdo->prepare("SELECT id FROM ai_rewrite_config WHERE company_id = ? LIMIT 1");
    $stmt->execute([$company_id]);
    $exist = $stmt->fetchColumn();

    if ($exist) {
        if (empty($vals)) { return true; }
        $set = [];
        $params = [];
        foreach ($vals as $k => $v) { $set[] = "`{$k}` = ?"; $params[] = $v; }
        $set[] = "updated_by = ?"; $params[] = (int)$user_id;
        $params[] = $company_id;
        $pdo->prepare("UPDATE ai_rewrite_config SET " . implode(', ', $set) . " WHERE company_id = ?")->execute($params);
    } else {
        $cfg = array_merge([
            'asr_provider' => 'openai_compatible',
            'asr_base_url' => 'https://api.siliconflow.cn/v1',
            'asr_api_key' => '',
            'asr_model' => 'FunAudioLLM/SenseVoiceSmall',
            'llm_provider' => 'openai_compatible',
            'llm_base_url' => 'https://api.deepseek.com/v1',
            'llm_api_key' => '',
            'llm_model' => 'deepseek-chat',
            'analyze_prompt' => $defaults['analyze'],
            'rewrite_prompt' => $defaults['rewrite'],
            'score_prompt'   => $defaults['score'],
            'translate_model' => ai_rw_default_translate_model(),
            'translate_prompt' => $defaults['translate'],
        ], $vals);
        $cols = array_merge(['company_id'], array_keys($cfg), ['updated_by']);
        $ph = array_fill(0, count($cols), '?');
        $params = array_merge([$company_id], array_values($cfg), [(int)$user_id]);
        $pdo->prepare("INSERT INTO ai_rewrite_config (`" . implode('`,`', $cols) . "`) VALUES (" . implode(',', $ph) . ")")
            ->execute($params);
    }
    return true;
}

/**
 * 判断配置是否可用于调用（有 key）。
 */
function ai_rw_config_ready($cfg) {
    return trim((string)$cfg['asr_api_key']) !== '' && trim((string)$cfg['llm_api_key']) !== '';
}

// ============================================================
// HTTP 基础
// ============================================================

/**
 * 由代理配置拼出 curl 选项（host 为空表示直连）。
 * type 支持 http（默认）/ socks5；用户名密码经 CURLOPT_PROXYUSERPWD 传递，避免 URL 拼接转义问题。
 */
function ai_rw_curl_proxy_opts($host, $type, $user, $pass) {
    $host = trim((string)$host);
    if ($host === '') { return []; }
    $t = strtolower(trim((string)$type));
    $opts = [
        CURLOPT_PROXY => $host,
        CURLOPT_PROXYTYPE => ($t === 'socks5' || $t === 'socks') ? CURLPROXY_SOCKS5_HOSTNAME : CURLPROXY_HTTP,
    ];
    $user = trim((string)$user);
    if ($user !== '') {
        $opts[CURLOPT_PROXYAUTH] = CURLAUTH_ANY;
        $opts[CURLOPT_PROXYUSERPWD] = $user . ':' . (string)$pass;
    }
    return $opts;
}

function ai_rw_http_post_json($url, $headers, $payload, $timeout = 120, $proxy_opts = []) {
    $ch = curl_init($url);
    $opts = [
        CURLOPT_POST => true,
        CURLOPT_POSTFIELDS => json_encode($payload, JSON_UNESCAPED_UNICODE),
        CURLOPT_HTTPHEADER => array_merge(['Content-Type: application/json'], $headers),
        CURLOPT_RETURNTRANSFER => true,
        CURLOPT_TIMEOUT => $timeout,
        CURLOPT_CONNECTTIMEOUT => 20,
        CURLOPT_SSL_VERIFYPEER => false,
        CURLOPT_SSL_VERIFYHOST => false,
    ];
    foreach ($proxy_opts as $k => $v) { $opts[$k] = $v; }
    curl_setopt_array($ch, $opts);
    $body = curl_exec($ch);
    $err  = curl_error($ch);
    $code = (int)curl_getinfo($ch, CURLINFO_HTTP_CODE);
    curl_close($ch);
    if ($body === false) { return ['ok' => false, 'code' => 0, 'body' => '', 'error' => 'cURL错误: ' . $err]; }
    return ['ok' => ($code >= 200 && $code < 300), 'code' => $code, 'body' => $body, 'error' => ''];
}

function ai_rw_http_post_multipart($url, $headers, $fields, $timeout = 300, $proxy_opts = []) {
    $ch = curl_init($url);
    $opts = [
        CURLOPT_POST => true,
        CURLOPT_POSTFIELDS => $fields,
        CURLOPT_HTTPHEADER => $headers,
        CURLOPT_RETURNTRANSFER => true,
        CURLOPT_TIMEOUT => $timeout,
        CURLOPT_CONNECTTIMEOUT => 20,
        CURLOPT_SSL_VERIFYPEER => false,
        CURLOPT_SSL_VERIFYHOST => false,
    ];
    foreach ($proxy_opts as $k => $v) { $opts[$k] = $v; }
    curl_setopt_array($ch, $opts);
    $body = curl_exec($ch);
    $err  = curl_error($ch);
    $code = (int)curl_getinfo($ch, CURLINFO_HTTP_CODE);
    curl_close($ch);
    if ($body === false) { return ['ok' => false, 'code' => 0, 'body' => '', 'error' => 'cURL错误: ' . $err]; }
    return ['ok' => ($code >= 200 && $code < 300), 'code' => $code, 'body' => $body, 'error' => ''];
}

/**
 * 判断链接类型：
 *   direct  —— 可直接下载的媒体直链（有媒体扩展名，或命中已知 CDN 域名）
 *   page    —— 网页地址（YouTube 观看页 / 抖音/TikTok 分享页等），curl 下载只会拿到 HTML
 *   unknown —— 无法判断，按直链尝试
 */
function ai_rw_classify_url($url) {
    $u = trim((string)$url);
    if (!preg_match('#^https?://#i', $u)) { return 'invalid'; }
    $host = strtolower((string)parse_url($u, PHP_URL_HOST));
    $path = (string)parse_url($u, PHP_URL_PATH);
    if (preg_match('#\.(mp4|m4a|mp3|wav|webm|aac|mov|flv|mkv|m4s)$#i', $path)) { return 'direct'; }
    if (preg_match('#(tiktokcdn|tiktokv|douyinvod|douyinpic|googlevideo|bytecdn|byteoversea|ibytedtos|snssdk|muscdn|ixigua|akamaized|cloudfront)#i', $host)) { return 'direct'; }
    if (preg_match('#(youtube\.com|youtu\.be|douyin\.com|tiktok\.com|bilibili\.com|kuaishou\.com|instagram\.com|xiaohongshu\.com)#i', $host)) { return 'page'; }
    return 'unknown';
}

/**
 * 下载视频/音频到本地（带重试）。返回本地路径。
 * 直链 CDN 可直接下载；网页地址会被提前拦截（见 ai_rw_run_asr）。
 */
function ai_rw_download_media($url, $dest_path, $max_bytes = 26214400, $timeout = 120, $attempts = 2) {
    $last = ['ok' => false, 'error' => '下载失败'];
    for ($i = 1; $i <= $attempts; $i++) {
        $r = ai_rw_download_once($url, $dest_path, $max_bytes, $timeout);
        if (!empty($r['ok'])) { $r['attempts'] = $i; return $r; }
        $last = $r;
        if (!empty($r['fatal'])) { break; }          // 不可重试：非媒体/超限/写入失败
        if ($i < $attempts) { sleep($i * 2); }        // 2s、4s 退避
    }
    $last['attempts'] = $attempts;
    return $last;
}

/**
 * 单次下载。区分「可重试」与「致命」错误。
 */
function ai_rw_download_once($url, $dest_path, $max_bytes, $timeout) {
    if (!preg_match('#^https?://#i', (string)$url)) {
        return ['ok' => false, 'fatal' => true, 'error' => '链接格式不正确（需 http/https）'];
    }
    $dir = dirname($dest_path);
    if (!is_dir($dir)) { @mkdir($dir, 0755, true); }

    $fp = @fopen($dest_path, 'wb');
    if (!$fp) { return ['ok' => false, 'fatal' => true, 'error' => '无法写入临时文件（目录权限？）']; }

    $too_big = false;
    $ch = curl_init($url);
    curl_setopt_array($ch, [
        CURLOPT_FILE => $fp,
        CURLOPT_FOLLOWLOCATION => true,
        CURLOPT_MAXREDIRS => 8,
        CURLOPT_TIMEOUT => $timeout,
        CURLOPT_CONNECTTIMEOUT => 20,
        CURLOPT_SSL_VERIFYPEER => false,
        CURLOPT_SSL_VERIFYHOST => false,
        CURLOPT_USERAGENT => 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36',
        CURLOPT_REFERER => 'https://www.tiktok.com/',
        CURLOPT_NOPROGRESS => false,
        CURLOPT_LOW_SPEED_LIMIT => 1024,   // 低于 1KB/s 持续 30s 视为卡死
        CURLOPT_LOW_SPEED_TIME => 30,
        CURLOPT_PROGRESSFUNCTION => function ($res, $dl_total, $dl_now) use ($max_bytes, &$too_big) {
            if ($dl_now > $max_bytes) { $too_big = true; return 1; }
            return 0;
        },
    ]);
    $ok = curl_exec($ch);
    $err = curl_error($ch);
    $code = (int)curl_getinfo($ch, CURLINFO_HTTP_CODE);
    $ctype = strtolower((string)curl_getinfo($ch, CURLINFO_CONTENT_TYPE));
    curl_close($ch);
    fclose($fp);

    if ($too_big) { @unlink($dest_path); return ['ok' => false, 'fatal' => true, 'error' => '视频超过 25MB，ASR 接口无法接收']; }
    if (!$ok || $code < 200 || $code >= 300) {
        @unlink($dest_path);
        // 5xx / 408 / 429 / 网络错误 可重试；4xx 视为直链失效
        $retryable = ($code === 0 || $code >= 500 || $code === 408 || $code === 429);
        return ['ok' => false, 'fatal' => !$retryable, 'error' => '下载失败 HTTP ' . $code . ($err ? ' ' . $err : '')];
    }

    $size = @filesize($dest_path);
    if (!$size) { @unlink($dest_path); return ['ok' => false, 'error' => '下载内容为空，链接可能已失效']; }

    // 内容嗅探：拿到的是 HTML/JSON 说明这是网页地址，不是媒体文件
    if (strpos($ctype, 'text/html') !== false || strpos($ctype, 'application/json') !== false) {
        @unlink($dest_path);
        return ['ok' => false, 'fatal' => true, 'error' => '该链接返回的是网页/接口内容，不是视频文件'];
    }
    $head = @file_get_contents($dest_path, false, null, 0, 512);
    if ($head !== false && preg_match('#^\s*(<!doctype|<html|<\?xml|\{)#i', $head)) {
        @unlink($dest_path);
        return ['ok' => false, 'fatal' => true, 'error' => '该链接返回的是网页内容，不是视频文件'];
    }

    return ['ok' => true, 'path' => $dest_path, 'size' => $size];
}

// ============================================================
// ASR / LLM
// ============================================================

/**
 * 语音转文字。OpenAI 兼容 /audio/transcriptions（multipart）。
 * 直接把视频文件（mp4/m4a/mp3/wav/webm）POST 上去，无需 ffmpeg 转码。
 */
function ai_rw_asr_transcribe($cfg, $file_path) {
    $base = rtrim((string)$cfg['asr_base_url'], '/');
    if ($base === '') { return ['ok' => false, 'error' => '未配置 ASR 接口地址']; }
    if (trim((string)$cfg['asr_api_key']) === '') { return ['ok' => false, 'error' => '未配置 ASR API Key']; }
    if (!is_file($file_path)) { return ['ok' => false, 'error' => '音频文件不存在']; }

    $url = $base . '/audio/transcriptions';
    $fields = [
        'file' => new CURLFile($file_path),
        'model' => (string)$cfg['asr_model'],
        'response_format' => 'json',
    ];
    $headers = ['Authorization: Bearer ' . $cfg['asr_api_key']];
    $proxy = ai_rw_curl_proxy_opts(
        isset($cfg['asr_proxy']) ? $cfg['asr_proxy'] : '',
        isset($cfg['asr_proxy_type']) ? $cfg['asr_proxy_type'] : '',
        isset($cfg['asr_proxy_user']) ? $cfg['asr_proxy_user'] : '',
        isset($cfg['asr_proxy_pass']) ? $cfg['asr_proxy_pass'] : ''
    );

    $res = ai_rw_http_post_multipart($url, $headers, $fields, 300, $proxy);
    if (!$res['ok']) {
        $msg = ai_rw_short_api_error($res);
        return ['ok' => false, 'error' => 'ASR 调用失败：' . $msg];
    }
    $json = json_decode($res['body'], true);
    if (!is_array($json)) { return ['ok' => false, 'error' => 'ASR 返回非 JSON：' . mb_substr($res['body'], 0, 200)]; }

    $text = '';
    if (isset($json['text'])) { $text = (string)$json['text']; }
    elseif (isset($json['data']['text'])) { $text = (string)$json['data']['text']; }
    $text = trim($text);
    if ($text === '') { return ['ok' => false, 'error' => 'ASR 未识别出文字（可能是纯音乐或无口播）']; }
    return ['ok' => true, 'text' => $text];
}

/**
 * 调用 LLM：按 llm_provider 适配 OpenAI 兼容 /chat/completions 或 Anthropic /messages。
 * 代理按 llm_proxy 配置透传；Claude 官方 API 未配置代理时直接报错（换 IP 会封号，与客户端同规则）。
 */
function ai_rw_llm_chat($cfg, $system_prompt, $user_prompt, $timeout = 180) {
    $base = rtrim((string)$cfg['llm_base_url'], '/');
    if ($base === '') { return ['ok' => false, 'error' => '未配置 LLM 接口地址']; }
    if (trim((string)$cfg['llm_api_key']) === '') { return ['ok' => false, 'error' => '未配置 LLM API Key']; }

    $provider = strtolower(trim((string)($cfg['llm_provider'] ?? '')));
    $proxy = ai_rw_curl_proxy_opts(
        isset($cfg['llm_proxy']) ? $cfg['llm_proxy'] : '',
        isset($cfg['llm_proxy_type']) ? $cfg['llm_proxy_type'] : '',
        isset($cfg['llm_proxy_user']) ? $cfg['llm_proxy_user'] : '',
        isset($cfg['llm_proxy_pass']) ? $cfg['llm_proxy_pass'] : ''
    );
    if ($provider === 'anthropic' && empty($proxy)) {
        return ['ok' => false, 'error' => 'Claude API 必须走指定代理线路（直连可能封号），请在「AI 接口配置」填写 LLM 代理'];
    }

    if ($provider === 'anthropic') {
        // base 填 https://api.anthropic.com/v1（已含 /messages 则原样使用）
        $url = (strpos($base, '/messages') !== false) ? $base : $base . '/messages';
        $payload = [
            'model' => (string)$cfg['llm_model'],
            'max_tokens' => 8192,
            'temperature' => 0.8,
            'system' => $system_prompt,
            'messages' => [
                ['role' => 'user', 'content' => $user_prompt],
            ],
        ];
        $headers = ['x-api-key: ' . $cfg['llm_api_key'], 'anthropic-version: 2023-06-01'];
    } elseif ($provider === 'gemini') {
        // base 填 https://generativelanguage.googleapis.com/v1beta
        $url = $base . '/models/' . rawurlencode((string)$cfg['llm_model']) . ':generateContent?key=' . rawurlencode($cfg['llm_api_key']);
        $payload = [
            'system_instruction' => ['parts' => [['text' => $system_prompt]]],
            'contents' => [['role' => 'user', 'parts' => [['text' => $user_prompt]]]],
            'generationConfig' => ['temperature' => 0.8, 'maxOutputTokens' => 8192],
        ];
        $headers = [];
    } else {
        $url = $base . '/chat/completions';
        $payload = [
            'model' => (string)$cfg['llm_model'],
            'messages' => [
                ['role' => 'system', 'content' => $system_prompt],
                ['role' => 'user', 'content' => $user_prompt],
            ],
            'temperature' => 0.8,
            'stream' => false,
        ];
        $headers = ['Authorization: Bearer ' . $cfg['llm_api_key']];
    }

    $res = ai_rw_http_post_json($url, $headers, $payload, $timeout, $proxy);
    if (!$res['ok']) {
        $msg = ai_rw_short_api_error($res);
        return ['ok' => false, 'error' => 'LLM 调用失败：' . $msg];
    }
    $json = json_decode($res['body'], true);
    if (!is_array($json)) { return ['ok' => false, 'error' => 'LLM 返回非 JSON']; }

    $content = '';
    if ($provider === 'anthropic' && isset($json['content']) && is_array($json['content'])) {
        // Anthropic 响应：content 是 [{type:"text", text:"..."}] 块数组
        foreach ($json['content'] as $blk) {
            if (is_array($blk) && isset($blk['type']) && $blk['type'] === 'text' && isset($blk['text'])) {
                $content .= (string)$blk['text'];
            }
        }
    }
    if ($content === '' && $provider === 'gemini' && isset($json['candidates'][0]['content']['parts']) && is_array($json['candidates'][0]['content']['parts'])) {
        // Gemini 响应：candidates[0].content.parts[].text
        foreach ($json['candidates'][0]['content']['parts'] as $p) {
            if (is_array($p) && isset($p['text'])) { $content .= (string)$p['text']; }
        }
    }
    if ($content === '' && isset($json['choices'][0]['message']['content'])) { $content = (string)$json['choices'][0]['message']['content']; }
    if ($content === '' && isset($json['output']['text'])) { $content = (string)$json['output']['text']; }
    $content = trim($content);
    if ($content === '') { return ['ok' => false, 'error' => 'LLM 返回内容为空']; }

    $usage = isset($json['usage']) ? $json['usage'] : [];
    return ['ok' => true, 'content' => $content, 'usage' => $usage];
}

function ai_rw_short_api_error($res) {
    $body = (string)$res['body'];
    $json = json_decode($body, true);
    if (is_array($json)) {
        if (isset($json['error']['message'])) { return 'HTTP ' . $res['code'] . ' ' . mb_substr((string)$json['error']['message'], 0, 200); }
        if (isset($json['message'])) { return 'HTTP ' . $res['code'] . ' ' . mb_substr((string)$json['message'], 0, 200); }
        if (isset($json['error']) && is_string($json['error'])) { return 'HTTP ' . $res['code'] . ' ' . mb_substr($json['error'], 0, 200); }
    }
    $t = trim($body);
    if ($t === '') { $t = $res['error']; }
    return 'HTTP ' . $res['code'] . ' ' . mb_substr($t, 0, 200);
}

/**
 * 从 LLM 输出里抠出 JSON（容忍 ```json 包裹与前后废话）。
 */
function ai_rw_extract_json($text) {
    $text = trim((string)$text);
    $text = preg_replace('/^```(?:json)?\s*/i', '', $text);
    $text = preg_replace('/\s*```$/', '', $text);
    $decoded = json_decode($text, true);
    if (is_array($decoded)) { return $decoded; }

    $start = strpos($text, '{');
    $end = strrpos($text, '}');
    if ($start !== false && $end !== false && $end > $start) {
        $cand = substr($text, $start, $end - $start + 1);
        $decoded = json_decode($cand, true);
        if (is_array($decoded)) { return $decoded; }
    }
    return null;
}

// ============================================================
// 四步流水线
// ============================================================

function ai_rw_media_dir() {
    return __DIR__ . '/uploads/ai_rewrite';
}

/**
 * 步骤1：下载媒体 + ASR 转写。成功后删除本地媒体，节省磁盘。
 */
function ai_rw_run_asr($pdo, $task, $cfg) {
    $task_id = (int)$task['id'];
    $url = trim((string)$task['source_url']);
    if ($url === '') { return ['ok' => false, 'error' => '该视频没有可用的播放链接，请改用「手动粘贴转写」']; }

    // 网页地址无法直接下载：提前给出可操作的提示，避免跑到 ASR 才失败
    $kind = ai_rw_classify_url($url);
    if ($kind === 'invalid') { return ['ok' => false, 'error' => '链接格式不正确，请改用「手动粘贴转写」']; }
    if ($kind === 'page') {
        return ['ok' => false, 'manual_hint' => true,
                'error' => '该链接是网页地址（如 YouTube 观看页），无法直接下载视频。请点「✍️ 手动粘贴转写」把口播/字幕粘进来，后续分析、洗稿照常进行。'];
    }

    $ext = 'mp4';
    if (preg_match('/\.(mp3|m4a|wav|webm|aac)(\?|$)/i', $url, $m)) { $ext = strtolower($m[1]); }
    $dest = ai_rw_media_dir() . '/task_' . $task_id . '_' . substr(md5($url . microtime(true)), 0, 8) . '.' . $ext;

    $dl = ai_rw_download_media($url, $dest);
    if (empty($dl['ok'])) {
        $msg = rtrim((string)$dl['error'], '。');
        $msg .= '（已重试 ' . (int)($dl['attempts'] ?? 1) . ' 次）';
        return ['ok' => false, 'manual_hint' => true,
                'error' => $msg . ' 点「✍️ 手动粘贴转写」即可跳过下载继续分析。'];
    }

    $asr = ai_rw_asr_transcribe($cfg, $dl['path']);
    @unlink($dl['path']);  // 无论成败都清理临时媒体
    if (empty($asr['ok'])) {
        $asr['manual_hint'] = true;
        $asr['error'] = (string)$asr['error'] . '（也可点「✍️ 手动粘贴转写」跳过转写）';
    }
    return $asr;
}

/**
 * 人工粘贴转写：直接把口播/字幕原文写入任务，标记转写完成，后续分析与洗稿照常进行。
 * 用于下载失败或 ASR 不可用时保证流程不中断。
 */
function ai_rw_set_manual_transcript($pdo, $task_id, $company_id, $text) {
    $text = trim((string)$text);
    if ($text === '') { return ['ok' => false, 'error' => '转写内容不能为空']; }
    if (mb_strlen($text) < 10) { return ['ok' => false, 'error' => '转写内容太短，请粘贴完整口播文案']; }
    $task = ai_rw_get_task($pdo, $task_id, $company_id);
    if (!$task) { return ['ok' => false, 'error' => '任务不存在']; }

    $fields = [
        'asr_text'    => $text,
        'asr_status'  => 2,
        'asr_manual'  => 1,
        'error_msg'   => '',
        'retry_count' => 0,
    ];
    // 若分析与洗稿此前已终态失败，一并重置以便继续
    if ((int)$task['analyze_status'] === 3) { $fields['analyze_status'] = 0; }
    if ((int)$task['rewrite_status'] === 3) { $fields['rewrite_status'] = 0; }
    ai_rw_task_update($pdo, $task_id, $fields);
    return ['ok' => true];
}

/**
 * 步骤2：爆点分析。
 */
function ai_rw_run_analyze($cfg, $asr_text, $meta) {
    $sys = (string)$cfg['analyze_prompt'];
    $user = "【平台】" . ($meta['platform'] ?? '') . "\n"
        . "【播放量】" . ($meta['play_count'] ?? 0) . "\n"
        . "【点赞】" . ($meta['digg_count'] ?? 0) . "　【评论】" . ($meta['comment_count'] ?? 0) . "\n"
        . "【原视频描述】" . mb_substr((string)($meta['title'] ?? ''), 0, 300) . "\n\n"
        . "【口播文案】\n" . $asr_text;
    $res = ai_rw_llm_chat($cfg, $sys, $user, 180);
    if (!$res['ok']) { return $res; }
    $json = ai_rw_extract_json($res['content']);
    if ($json === null) { return ['ok' => false, 'error' => '爆点分析返回的不是合法 JSON', 'raw' => $res['content']]; }
    return ['ok' => true, 'data' => $json, 'usage' => $res['usage']];
}

/**
 * 步骤3：洗稿出文案。
 */
function ai_rw_run_rewrite($cfg, $asr_text, $analysis, $meta) {
    $sys = (string)$cfg['rewrite_prompt'];
    $pointText = json_encode($analysis, JSON_UNESCAPED_UNICODE | JSON_PRETTY_PRINT);
    $user = "【赛道】" . ($meta['category'] ?? '') . "\n"
        . "【目标时长】" . (($meta['target_seconds'] ?? 45)) . " 秒\n\n"
        . "【爆款核心】\n" . $pointText . "\n\n"
        . "【原口播稿】\n" . $asr_text;
    $res = ai_rw_llm_chat($cfg, $sys, $user, 180);
    if (!$res['ok']) { return $res; }
    $json = ai_rw_extract_json($res['content']);
    if ($json === null) { return ['ok' => false, 'error' => '洗稿返回的不是合法 JSON', 'raw' => $res['content']]; }
    return ['ok' => true, 'data' => $json, 'usage' => $res['usage']];
}

/**
 * 把洗稿结果拼成可直接朗读的整段文案。
 */
function ai_rw_compose_script_text($data) {
    $parts = [];
    if (!empty($data['hook'])) { $parts[] = (string)$data['hook']; }
    if (!empty($data['body'])) {
        if (is_array($data['body'])) {
            foreach ($data['body'] as $p) { $parts[] = (string)$p; }
        } else {
            $parts[] = (string)$data['body'];
        }
    }
    if (!empty($data['cta'])) { $parts[] = (string)$data['cta']; }
    return implode("\n", $parts);
}

/**
 * 步骤4：文案打分（0-100）并给出改进建议。
 * 供人工筛选后对指定文案重新评分，不修改原任务流程。
 */
function ai_rw_run_score($cfg, $script_text, $meta = []) {
    $sys = (string)$cfg['score_prompt'];
    $user = "【赛道】" . (isset($meta['category']) ? $meta['category'] : '') . "\n"
        . "【标题】" . (isset($meta['title']) ? $meta['title'] : '') . "\n\n"
        . "【洗稿文案】\n" . $script_text;
    $res = ai_rw_llm_chat($cfg, $sys, $user, 120);
    if (!$res['ok']) { return $res; }
    $json = ai_rw_extract_json($res['content']);
    if ($json === null) { return ['ok' => false, 'error' => '打分返回的不是合法 JSON', 'raw' => $res['content']]; }
    $score = isset($json['score']) ? (int)$json['score'] : 0;
    if ($score < 0) { $score = 0; }
    if ($score > 100) { $score = 100; }
    $json['score'] = $score;
    return ['ok' => true, 'data' => $json, 'usage' => isset($res['usage']) ? $res['usage'] : []];
}

/**
 * 把打分结果写回文案记录（score / score_json / scored_at）。
 */
function ai_rw_save_score($pdo, $script_id, $data) {
    $score = isset($data['score']) ? (int)$data['score'] : 0;
    if ($score < 0) { $score = 0; }
    if ($score > 100) { $score = 100; }
    $pdo->prepare("UPDATE ai_scripts SET score = ?, score_json = ?, scored_at = NOW(), updated_at = NOW() WHERE id = ?")
        ->execute([$score, json_encode($data, JSON_UNESCAPED_UNICODE), (int)$script_id]);
    return $score;
}

// ============================================================
// 翻译（供人工对照：原文 / 译文 / 洗稿）
// ============================================================

/**
 * 解析翻译接口参数：默认复用 ASR 的地址与 Key（同属硅基流动，Hunyuan-MT 免费）。
 */
function ai_rw_translate_endpoint($cfg) {
    $base = trim((string)($cfg['translate_base_url'] ?? ''));
    if ($base === '') { $base = (string)($cfg['asr_base_url'] ?? ''); }
    $key = trim((string)($cfg['translate_api_key'] ?? ''));
    if ($key === '') { $key = (string)($cfg['asr_api_key'] ?? ''); }
    $model = trim((string)($cfg['translate_model'] ?? ''));
    if ($model === '') { $model = ai_rw_default_translate_model(); }
    return ['base_url' => rtrim($base, '/'), 'api_key' => $key, 'model' => $model];
}

/**
 * 调用翻译模型（OpenAI 兼容 /chat/completions）。
 * 说明：Hunyuan-MT 默认目标语言为中文；system 指令丢失时，user 即源文，仍会翻成中文。
 *
 * @param string $target 目标语言，默认「中文」
 */
function ai_rw_run_translate($cfg, $text, $target = '中文') {
    $text = trim((string)$text);
    if ($text === '') { return ['ok' => false, 'error' => '原文为空，无法翻译']; }

    $ep = ai_rw_translate_endpoint($cfg);
    if ($ep['base_url'] === '') { return ['ok' => false, 'error' => '未配置翻译接口地址（可留空复用 ASR 地址）']; }
    if ($ep['api_key'] === '')  { return ['ok' => false, 'error' => '未配置翻译 API Key（可留空复用 ASR Key）']; }

    $sys = trim((string)($cfg['translate_prompt'] ?? ''));
    if ($sys === '') { $defaults = ai_rw_default_prompts(); $sys = $defaults['translate']; }
    $sys = str_replace('{目标语言}', $target, $sys);

    $url = $ep['base_url'] . '/chat/completions';
    $payload = [
        'model' => $ep['model'],
        'messages' => [
            ['role' => 'system', 'content' => $sys],
            ['role' => 'user', 'content' => $text],
        ],
        'temperature' => 0.3,
        'stream' => false,
    ];
    // 翻译走 ASR 的代理线路（与客户端 credentials 下发约定一致）
    $proxy = ai_rw_curl_proxy_opts(
        isset($cfg['asr_proxy']) ? $cfg['asr_proxy'] : '',
        isset($cfg['asr_proxy_type']) ? $cfg['asr_proxy_type'] : '',
        isset($cfg['asr_proxy_user']) ? $cfg['asr_proxy_user'] : '',
        isset($cfg['asr_proxy_pass']) ? $cfg['asr_proxy_pass'] : ''
    );
    $res = ai_rw_http_post_json($url, ['Authorization: Bearer ' . $ep['api_key']], $payload, 120, $proxy);
    if (!$res['ok']) { return ['ok' => false, 'error' => '翻译调用失败：' . ai_rw_short_api_error($res)]; }

    $json = json_decode($res['body'], true);
    if (!is_array($json)) { return ['ok' => false, 'error' => '翻译返回非 JSON']; }
    $content = '';
    if (isset($json['choices'][0]['message']['content'])) { $content = (string)$json['choices'][0]['message']['content']; }
    elseif (isset($json['output']['text'])) { $content = (string)$json['output']['text']; }
    $content = trim($content);
    if ($content === '') { return ['ok' => false, 'error' => '翻译返回内容为空']; }
    return ['ok' => true, 'text' => $content, 'usage' => isset($json['usage']) ? $json['usage'] : []];
}

/**
 * 把译文写回任务（field: asr_text_zh | rewrite_text_zh）。
 */
function ai_rw_save_translation($pdo, $task_id, $field, $text) {
    if (!in_array($field, ['asr_text_zh', 'rewrite_text_zh'], true)) { return false; }
    return $pdo->prepare("UPDATE ai_rewrite_tasks SET `{$field}` = ?, updated_at = NOW() WHERE id = ?")
        ->execute([(string)$text, (int)$task_id]);
}

// ============================================================
// 入库
// ============================================================

/**
 * 从视频标题提取作品名（标题兜底，LLM 未识别出 work 时使用）。
 * 只认两种高置信模式：《作品名》、"作品名第N集/话/季/期"。
 */
function ai_rw_extract_work_from_title($title) {
    $title = trim((string)$title);
    if ($title === '') { return ''; }
    if (preg_match('/《(.+?)》/u', $title, $m)) { return mb_substr(trim($m[1]), 0, 100); }
    if (preg_match('/^(.{2,30}?)(?:\s*第\s*\d+\s*[集话季期部])/u', $title, $m)) { return trim($m[1]); }
    return '';
}

/**
 * 把爆点分析沉淀进爆款核心库（按 task_id 唯一）。
 */
function ai_rw_upsert_viral_point($pdo, $task, $analysis) {
    $points = isset($analysis['points']) ? $analysis['points'] : [];
    $structure = isset($analysis['structure']) ? $analysis['structure'] : [];
    $keywords = isset($analysis['keywords']) ? $analysis['keywords'] : [];
    if (is_array($points)) { $points = implode("\n", array_map('strval', $points)); }
    if (is_array($keywords)) { $keywords = implode(',', array_map('strval', $keywords)); }

    // 作品标签：LLM 识别的规范名 | 别名...；识别不出时用标题提取兜底
    $work = trim((string)($analysis['work'] ?? ''));
    if ($work === '') { $work = ai_rw_extract_work_from_title($task['title'] ?? ''); }
    if ($work !== '' && isset($analysis['work_aliases']) && is_array($analysis['work_aliases'])) {
        $parts = [$work];
        foreach ($analysis['work_aliases'] as $a) {
            $a = trim((string)$a);
            if ($a !== '' && !in_array($a, $parts)) { $parts[] = $a; }
        }
        $work = implode('|', $parts);
    }
    $work = mb_substr($work, 0, 255);

    $stmt = $pdo->prepare("INSERT INTO viral_points
        (company_id, task_id, video_id, platform, category, work, title, source_url, play_count,
         hook, structure_json, points_json, emotion, keywords, reusable, summary, asr_text)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        ON DUPLICATE KEY UPDATE
         category=VALUES(category), work=VALUES(work), title=VALUES(title), play_count=VALUES(play_count),
         hook=VALUES(hook), structure_json=VALUES(structure_json), points_json=VALUES(points_json),
         emotion=VALUES(emotion), keywords=VALUES(keywords), reusable=VALUES(reusable),
         summary=VALUES(summary), asr_text=VALUES(asr_text), updated_at=NOW()");
    $stmt->execute([
        (int)$task['company_id'],
        (int)$task['id'],
        (string)$task['video_id'],
        (string)$task['platform'],
        (string)($analysis['category'] ?? ''),
        $work,
        mb_substr((string)($task['title'] ?? ''), 0, 255),
        (string)$task['source_url'],
        (int)$task['play_count'],
        mb_substr((string)($analysis['hook'] ?? ''), 0, 500),
        json_encode($structure, JSON_UNESCAPED_UNICODE),
        $points,
        mb_substr((string)($analysis['emotion'] ?? ''), 0, 100),
        mb_substr($keywords, 0, 500),
        mb_substr((string)($analysis['reusable'] ?? ''), 0, 500),
        (string)($analysis['summary'] ?? ''),
        (string)$task['asr_text'],
    ]);

    $s = $pdo->prepare("SELECT id FROM viral_points WHERE task_id = ? LIMIT 1");
    $s->execute([(int)$task['id']]);
    return (int)$s->fetchColumn();
}

/**
 * 洗稿结果写入文案库。
 */
function ai_rw_save_script($pdo, $task, $data, $point_id = 0) {
    $tags = isset($data['tags']) ? $data['tags'] : [];
    $kept = isset($data['kept_points']) ? $data['kept_points'] : [];
    if (is_array($tags)) { $tags = implode(',', array_map('strval', $tags)); }
    if (is_array($kept)) { $kept = implode("\n", array_map('strval', $kept)); }

    $body = isset($data['body']) ? $data['body'] : [];
    if (is_array($body)) { $body = implode("\n", array_map('strval', $body)); }

    $full = ai_rw_compose_script_text($data);

    $pdo->prepare("INSERT INTO ai_scripts
        (company_id, user_id, task_id, point_id, category, title, hook, body, cta, tags, kept_points, full_text, status)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,0)")
        ->execute([
            (int)$task['company_id'],
            (int)$task['user_id'],
            (int)$task['id'],
            (int)$point_id,
            (string)$task['category'],
            mb_substr((string)($data['title'] ?? ''), 0, 255),
            mb_substr((string)($data['hook'] ?? ''), 0, 500),
            $body,
            mb_substr((string)($data['cta'] ?? ''), 0, 500),
            mb_substr($tags, 0, 500),
            $kept,
            $full,
        ]);
    return (int)$pdo->lastInsertId();
}

// ============================================================
// 任务状态维护
// ============================================================

function ai_rw_task_update($pdo, $task_id, $fields) {
    if (empty($fields)) { return; }
    $set = [];
    $params = [];
    foreach ($fields as $k => $v) { $set[] = "`{$k}` = ?"; $params[] = $v; }
    $params[] = (int)$task_id;
    $pdo->prepare("UPDATE ai_rewrite_tasks SET " . implode(', ', $set) . ", updated_at = NOW() WHERE id = ?")
        ->execute($params);
}

function ai_rw_get_task($pdo, $task_id, $company_id = 0) {
    $sql = "SELECT * FROM ai_rewrite_tasks WHERE id = ?";
    $params = [(int)$task_id];
    if ($company_id > 0) { $sql .= " AND company_id = ?"; $params[] = (int)$company_id; }
    $stmt = $pdo->prepare($sql . " LIMIT 1");
    $stmt->execute($params);
    return $stmt->fetch(PDO::FETCH_ASSOC);
}

/**
 * 失败标记：retry_count +1，达到上限置为终态失败(3)，否则回到待处理(0)。
 */
function ai_rw_mark_fail($pdo, $task_id, $status_field, $error, $max_retry = 3) {
    $task = ai_rw_get_task($pdo, $task_id);
    if (!$task) { return; }
    $retry = (int)$task['retry_count'] + 1;
    $final = ($retry >= $max_retry) ? 3 : 0;
    ai_rw_task_update($pdo, $task_id, [
        $status_field => $final,
        'retry_count' => $retry,
        'error_msg' => mb_substr((string)$error, 0, 500),
    ]);
}

function ai_rw_get_point_id_by_task($pdo, $task_id) {
    $s = $pdo->prepare("SELECT id FROM viral_points WHERE task_id = ? LIMIT 1");
    $s->execute([(int)$task_id]);
    return (int)$s->fetchColumn();
}

/**
 * 推进一个任务到"下一步"，每次调用只执行一个阶段。
 * 供前端 AJAX 轮询与 cron 队列共用，避免单请求长时间阻塞。
 *
 * 返回：
 *   ['ok'=>true,  'step'=>'asr|analyze|rewrite|none', 'done'=>bool]
 *   ['ok'=>false, 'step'=>'asr|analyze|rewrite', 'error'=>'...', 'need_config'=>bool]
 */
function ai_rw_process_task_step($pdo, $task_id, $cfg = null) {
    ai_rw_ensure_tables($pdo);
    $task = ai_rw_get_task($pdo, $task_id);
    if (!$task) { return ['ok' => false, 'step' => '', 'error' => '任务不存在']; }
    if ($cfg === null) { $cfg = ai_rw_get_config($pdo, $task['company_id']); }
    if (!ai_rw_config_ready($cfg)) {
        return ['ok' => false, 'step' => '', 'error' => 'AI 配置未完成：ASR 与 LLM 的 API Key 均为必填', 'need_config' => true];
    }

    foreach (['asr_status', 'analyze_status', 'rewrite_status'] as $sf) {
        if ((int)$task[$sf] === 3) {
            return ['ok' => false, 'step' => '', 'error' => '任务已失败（' . ai_rw_status_text(3) . '），请先重试'];
        }
    }

    // 步骤1：下载媒体 + ASR 转写
    if ((int)$task['asr_status'] === 0) {
        ai_rw_task_update($pdo, $task_id, ['asr_status' => 1, 'error_msg' => '']);
        $res = ai_rw_run_asr($pdo, $task, $cfg);
        if (empty($res['ok'])) {
            ai_rw_mark_fail($pdo, $task_id, 'asr_status', $res['error']);
            return ['ok' => false, 'step' => 'asr', 'error' => $res['error']];
        }
        ai_rw_task_update($pdo, $task_id, ['asr_text' => $res['text'], 'asr_status' => 2, 'retry_count' => 0]);
        return ['ok' => true, 'step' => 'asr', 'done' => false];
    }

    // 步骤2：爆点分析 + 爆款核心入库
    if ((int)$task['asr_status'] === 2 && (int)$task['analyze_status'] === 0) {
        ai_rw_task_update($pdo, $task_id, ['analyze_status' => 1]);
        $res = ai_rw_run_analyze($cfg, (string)$task['asr_text'], $task);
        if (empty($res['ok'])) {
            ai_rw_mark_fail($pdo, $task_id, 'analyze_status', $res['error']);
            return ['ok' => false, 'step' => 'analyze', 'error' => $res['error']];
        }
        $data = $res['data'];
        $cat = isset($data['category']) ? mb_substr((string)$data['category'], 0, 50) : '';
        ai_rw_task_update($pdo, $task_id, [
            'analyze_json' => json_encode($data, JSON_UNESCAPED_UNICODE),
            'analyze_status' => 2,
            'category' => $cat,
            'retry_count' => 0,
        ]);
        $task['category'] = $cat;
        ai_rw_upsert_viral_point($pdo, $task, $data);
        return ['ok' => true, 'step' => 'analyze', 'done' => false];
    }

    // 步骤3：洗稿出文案 + 文案入库
    if ((int)$task['analyze_status'] === 2 && (int)$task['rewrite_status'] === 0) {
        ai_rw_task_update($pdo, $task_id, ['rewrite_status' => 1]);
        $analysis = ai_rw_extract_json((string)$task['analyze_json']);
        if ($analysis === null) { $analysis = []; }
        $meta = $task;
        if (!isset($meta['target_seconds'])) { $meta['target_seconds'] = 45; }
        $res = ai_rw_run_rewrite($cfg, (string)$task['asr_text'], $analysis, $meta);
        if (empty($res['ok'])) {
            ai_rw_mark_fail($pdo, $task_id, 'rewrite_status', $res['error']);
            return ['ok' => false, 'step' => 'rewrite', 'error' => $res['error']];
        }
        $data = $res['data'];
        ai_rw_task_update($pdo, $task_id, [
            'rewrite_json' => json_encode($data, JSON_UNESCAPED_UNICODE),
            'rewrite_text' => ai_rw_compose_script_text($data),
            'rewrite_status' => 2,
            'retry_count' => 0,
        ]);
        ai_rw_save_script($pdo, $task, $data, ai_rw_get_point_id_by_task($pdo, $task_id));
        return ['ok' => true, 'step' => 'rewrite', 'done' => true];
    }

    return ['ok' => true, 'step' => 'none', 'done' => true];
}

/**
 * 任务是否已全部完成。
 */
function ai_rw_task_is_done($task) {
    return (int)$task['asr_status'] === 2 && (int)$task['analyze_status'] === 2 && (int)$task['rewrite_status'] === 2;
}

/**
 * 状态文案映射（前端展示用）。
 */
function ai_rw_status_text($code) {
    switch ((int)$code) {
        case 0: return '待处理';
        case 1: return '处理中';
        case 2: return '已完成';
        case 3: return '失败';
        default: return '未知';
    }
}