import glob, io, json, re

translations = {
    "en": {"savedNow": "Data directory set. It is active now.", "migrateData": "Move existing data here", "waitingData": "Existing data will be moved from {{from}} into"},
    "es": {"savedNow": "Directorio de datos establecido. Ya está activo.", "migrateData": "Mover los datos existentes aquí", "waitingData": "Los datos existentes se moverán desde {{from}} a"},
    "fr": {"savedNow": "Dossier de données défini. Il est actif maintenant.", "migrateData": "Déplacer les données existantes ici", "waitingData": "Les données existantes seront déplacées de {{from}} vers"},
    "ja": {"savedNow": "データディレクトリを設定しました。今すぐ有効です。", "migrateData": "既存のデータをここに移動", "waitingData": "既存データは {{from}} から次へ移動します："},
    "ko": {"savedNow": "데이터 디렉터리를 설정했습니다. 지금 바로 적용됩니다.", "migrateData": "기존 데이터를 여기로 이동", "waitingData": "기존 데이터가 {{from}}에서 다음으로 이동됩니다:"},
    "pt": {"savedNow": "Diretório de dados definido. Já está ativo.", "migrateData": "Mover os dados existentes para cá", "waitingData": "Os dados existentes serão movidos de {{from}} para"},
    "ru": {"savedNow": "Папка данных задана. Она активна сейчас.", "migrateData": "Переместить существующие данные сюда", "waitingData": "Существующие данные будут перемещены из {{from}} в"},
    "tr": {"savedNow": "Veri dizini ayarlandı. Şu an etkin.", "migrateData": "Mevcut verileri buraya taşı", "waitingData": "Mevcut veriler {{from}} konumundan şuraya taşınacak"},
    "vi": {"savedNow": "Đã đặt thư mục dữ liệu. Nó hoạt động ngay bây giờ.", "migrateData": "Di chuyển dữ liệu hiện có vào đây", "waitingData": "Dữ liệu hiện có sẽ được di chuyển từ {{from}} đến"},
    "zh": {"savedNow": "数据目录已设置，立即生效。", "migrateData": "把现有数据移入此目录", "waitingData": "现有数据将从 {{from}} 移入"},
}

files = sorted(glob.glob("src/i18n/locales/*.json"))
for path in files:
    lang = path.rsplit("/", 1)[-1].rsplit("\\", 1)[-1].rsplit(".", 1)[0]
    if lang not in translations:
        continue
    lines = io.open(path, encoding="utf-8").read().split("\n")
    open_i = next(i for i, l in enumerate(lines) if re.search(r'"dataRoot"\s*:\s*\{', l))
    anchor = next(j for j in range(open_i, open_i + 80) if re.search(r'"move"\s*:', lines[j]))
    indent = lines[anchor].split('"')[0]
    block = "".join(f"{indent}\"{k}\": {json.dumps(v, ensure_ascii=False)},\n" for k, v in translations[lang].items())
    lines[anchor + 1:anchor + 1] = [block.rstrip("\n")]
    io.open(path, "w", encoding="utf-8").write("\n".join(lines))
    print(f"updated {lang}")

ref = json.load(io.open("src/i18n/locales/en.json", encoding="utf-8"))["settings"]["dataRoot"]
for path in files:
    lang = path.rsplit("/", 1)[-1].rsplit("\\", 1)[-1].rsplit(".", 1)[0]
    if lang not in translations:
        continue
    data = json.load(io.open(path, encoding="utf-8"))["settings"]["dataRoot"]
    missing = set(ref) - set(data)
    extra = set(data) - set(ref)
    print(f"parity {lang}: missing={sorted(missing)} extra={sorted(extra)}" if missing or extra else f"parity ok {lang}")