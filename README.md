# VidDupe

**高性能動画重複検出・安全削除ツール**

VidDupeは、大規模な動画コレクション（5TB+/1000+ファイル）から重複動画を高精度で検出し、安全に削除するRust製CLIツールです。段階的解析アプローチとSQLiteキャッシュにより、現実的な処理時間で動作します。

## ✨ 特徴

- **🔍 多段階重複検出**
  - Stage 0: メタデータ + 先頭/末尾ハッシュによる高速事前フィルタ
  - Stage 1: フレーム知覚ハッシュ（pHash）による視覚的類似度判定
  - Stage 2: Chromaprint音声フィンガープリント（オプション）
- **⚡ 高性能・スケーラブル**
  - **リアルタイムETA表示** - 作業中に終了予定時刻を表示
  - **処理速度監視** - files/sec でリアルタイム性能確認
  - 最適化されたバッチデータベース処理
  - 並列処理とセマフォによる負荷制御
  - SQLiteキャッシュで増分処理（WAL mode + 128MB cache）
  - 5TB/1000+ファイルでも現実的な処理時間
- **🛡️ 安全第一**
  - 既定はドライラン（`--apply`必須）
  - 既定はゴミ箱移動（`--hard-delete`で物理削除）
  - 対話式削除確認
  - 完全な削除ログ記録
- **🎯 高精度**
  - 保守的な閾値設定で誤判定を最小化
  - 複数検出手法の組み合わせで信頼性向上
- **🖥️ クロスプラットフォーム**
  - Windows / macOS / Linux対応
  - .gitignore/.ignore ファイル尊重

## 📦 インストール

### 前提条件

以下のツールが必要です：

#### FFmpeg（必須）
```bash
# Windows (Chocolatey)
choco install ffmpeg

# Windows (winget)
winget install Gyan.FFmpeg

# macOS (Homebrew)
brew install ffmpeg

# Ubuntu/Debian
sudo apt install ffmpeg

# CentOS/RHEL
sudo yum install ffmpeg

# Arch Linux
sudo pacman -S ffmpeg
```

#### Chromaprint（推奨）
音声フィンガープリント機能のために推奨（なくても動作可能）：

```bash
# Windows (Chocolatey)
choco install chromaprint

# macOS (Homebrew)
brew install chromaprint

# Ubuntu/Debian
sudo apt install chromaprint-tools

# CentOS/RHEL
sudo yum install chromaprint-tools

# Arch Linux
sudo pacman -S chromaprint
```

### VidDupeのビルド

```bash
# このリポジトリをクローン
git clone <repository-url>
cd viddupe

# リリースビルド
cargo build --release

# インストール
cargo install --path .
```

## 🚀 使用方法

### 基本ワークフロー

1. **スキャン** - 動画ファイルを探索してキャッシュ構築
2. **確認** - 検出された重複を確認
3. **削除** - 対話式で安全に重複削除

### コマンド一覧

#### 1. ファイルスキャン
```bash
# 基本スキャン（.mp4ファイル）
viddupe scan /path/to/videos

# 複数拡張子でスキャン
viddupe scan /path/to/videos --extensions mp4,mkv,m4v

# 詳細オプション
viddupe scan /path/to/videos \
    --extensions mp4,mkv,avi \
    --exclude ".*backup.*" \
    --min-size-mb 10 \
    --jobs 4
```

#### 2. 重複表示
```bash
# 重複一覧表示
viddupe dupes /path/to/videos

# 詳細情報付き
viddupe dupes /path/to/videos --details
```

#### 3. 重複削除（対話式）
```bash
# ドライラン（安全確認）
viddupe prune /path/to/videos

# 実際に削除（ゴミ箱へ）
viddupe prune /path/to/videos --apply

# 永久削除（危険！）
viddupe prune /path/to/videos --apply --hard-delete

# 自動選択モード
viddupe prune /path/to/videos --apply --auto-keep best
viddupe prune /path/to/videos --apply --auto-keep smallest
viddupe prune /path/to/videos --apply --auto-keep oldest
viddupe prune /path/to/videos --apply --auto-keep newest
```

#### 4. キャッシュ管理
```bash
# キャッシュクリア
viddupe clear-cache

# カスタムDBパス
viddupe scan /path/to/videos --db /custom/path/cache.db
```

### 高度なオプション

```bash
# 並列処理調整
viddupe scan /path/to/videos --jobs 8 --ffmpeg-par 3

# 検出閾値調整（上級者向け）
viddupe prune /path/to/videos \
    --threshold-phash-avg 4 \
    --threshold-phash-max 8

# フレームサンプリング調整
viddupe scan /path/to/videos --frame-samples 15 --head-tail-mib 32

# 除外パターン
viddupe scan /path/to/videos \
    --exclude ".*temp.*" \
    --exclude ".*/backup/.*"
```

## 📊 出力例

### リアルタイム進捗表示（NEW!）
```
⠁ [00:02:15] [████████████████████░░░░] 1,234/1,500 files (8.2/s) | ETA: 32s | Processing: vacation_2023.mp4
```

**進捗表示の見方:**
- `⠁` - アニメーション回転インジケーター
- `[00:02:15]` - 経過時間
- `████████████████████░░░░` - 進捗バー
- `1,234/1,500 files` - 処理済み/総ファイル数
- `(8.2/s)` - **リアルタイム処理速度**
- `ETA: 32s` - **推定残り時間**
- `Processing: vacation_2023.mp4` - 現在処理中のファイル

### 最終処理サマリー（NEW!）
```
Processing completed: 1,234 files in 145.3s (8.5 files/sec)
```

### 重複クラスター表示
```
Found 3 duplicate clusters:

--- Cluster 1 (2 files) ---
  Similarity: 95.2% confidence
  Total size: 2.1 GB
  👑 /videos/movie_1080p.mp4 (1.2 GB | 2023-10-15 14:30)
     /videos/movie_720p.mp4 (892 MB | 2023-10-10 09:15)
  💡 Recommended to keep: /videos/movie_1080p.mp4

--- Cluster 2 (3 files) ---
  Similarity: 88.7% confidence  
  Total size: 4.5 GB
  👑 /videos/concert_original.mkv (2.1 GB | 2023-09-20 18:45)
     /videos/concert_copy1.mp4 (1.2 GB | 2023-09-21 10:20)
     /videos/concert_copy2.avi (1.2 GB | 2023-09-22 15:10)
  💡 Recommended to keep: /videos/concert_original.mkv
```

### 削除確認画面
```
=== Duplicate Cluster 1 ===
Detection: perceptual_hash+audio_fingerprint (confidence: 95.2%)

Which file do you want to KEEP?
> 1. /videos/movie_1080p.mp4 (1.2 GB, 2023-10-15) ⭐ (recommended)
  2. /videos/movie_720p.mp4 (892 MB, 2023-10-10)
  3. Keep all files (skip this cluster)

📋 Files to be deleted from this cluster:
  ❌ /videos/movie_720p.mp4 (892 MB)
  💾 Total space to free: 892 MB
```

## ⚙️ 設定とチューニング

### 性能最適化

#### 並列処理設定
- `--jobs`: ファイル処理の並列数（既定: 4）
- `--ffmpeg-par`: ffmpeg同時起動数（既定: 2）
- CPU集約的処理とI/O処理のバランスを調整

#### ストレージ推奨設定
- **SSD推奨**: 大量のランダムI/Oが発生
- **十分な空き容量**: 処理中に一時ファイル作成
- **データベース配置**: 高速ストレージ上に配置

#### メモリ使用量
- SQLiteキャッシュ: 128MB（最適化済み）
- mmap: 512MB（大規模ファイル処理用）
- 1000ファイルあたり約100MB程度のメモリ使用

#### 新パフォーマンス機能（v0.2.0+）
- **バッチデータベース処理**: トランザクションコストを削減
- **WALモード**: 読み書き並列性向上
- **リアルタイムETA**: 残り時間を動的計算・表示
- **処理速度監視**: files/sec で性能をリアルタイム確認

### 検出精度調整

#### 閾値設定（上級者向け）
```bash
# より厳格な検出（誤検出を抑制）
--threshold-phash-avg 3 --threshold-phash-max 6

# より寛容な検出（見逃しを抑制）  
--threshold-phash-avg 8 --threshold-phash-max 15
```

#### フレーム抽出設定
```bash
# より多くのフレームサンプリング（精度向上）
--frame-samples 20

# 大きなハッシュチャンク（大きなファイル用）
--head-tail-mib 64
```

## 🚨 重要な注意点

### 安全性について
- **必ずバックアップ**: 重要なファイルは事前にバックアップ
- **ドライランで確認**: `--apply`を付けずに必ず事前確認
- **段階的処理**: 小さなセットでテストしてから大規模実行
- **ゴミ箱を活用**: `--hard-delete`は慎重に使用

### 制限事項
- **暗号化されたファイル**: 正常に解析できません
- **破損ファイル**: エラーログに記録されスキップ
- **権限不足**: 読み書き権限必要
- **ネットワークドライブ**: パフォーマンス低下の可能性

### パフォーマンス考慮
- **初回実行**: 全ファイル解析のため時間がかかる
- **増分実行**: 2回目以降はキャッシュ利用で高速
- **大容量ファイル**: 処理時間とメモリ使用量増加

## 🔧 トラブルシューティング

### よくある問題

#### 1. ffmpegが見つからない
```
Error: External tool check failed: Required dependency 'ffprobe' not found
```
**解決方法**: 前提条件セクションに従ってFFmpegをインストール

#### 2. 権限エラー
```
Error: Failed to read metadata for /path/file.mp4: Permission denied
```
**解決方法**: 
- ファイルの読み取り権限確認
- 管理者権限で実行（必要な場合）

#### 3. データベースロックエラー
```
Error: Database is locked
```
**解決方法**:
- 他のVidDupeプロセスが実行中でないか確認
- `viddupe clear-cache`でキャッシュリセット

#### 4. メモリ不足
```
Error: failed to allocate memory
```
**解決方法**:
- `--jobs`と`--ffmpeg-par`を減らす
- より小さなディレクトリ単位で処理

#### 5. 破損したキャッシュ
```
Error: Failed to parse ffprobe JSON output
```
**解決方法**:
```bash
# キャッシュを削除して再実行
viddupe clear-cache
viddupe scan /path/to/videos
```

### デバッグ情報

#### 詳細ログ
```bash
# デバッグモードで実行
viddupe --debug scan /path/to/videos

# 環境変数でログレベル設定
RUST_LOG=debug viddupe scan /path/to/videos
```

#### 処理状況確認
- **リアルタイム進捗バー**: ETA・処理速度・現在ファイル表示
- **詳細統計情報**: 処理完了時に総時間・平均速度表示
- `viddupe_deletions.csv`で削除ログ確認
- SQLiteツールでキャッシュ内容確認

## 📁 ファイル構成

実行後に作成されるファイル：
- `viddupe.db` - SQLiteキャッシュデータベース
- `viddupe_deletions.csv` - 削除操作ログ
- `viddupe.log` - 実行ログ（デバッグ時）

## 🧑‍💻 開発者向け情報

### アーキテクチャ
- **言語**: Rust 2021 Edition
- **主要クレート**: tokio, rusqlite, img_hash, dialoguer
- **並列処理**: rayon + tokio
- **データベース**: SQLite with WAL mode

### モジュール構成
```
src/
├── main.rs      # エントリポイント
├── cli.rs       # CLI定義
├── scan.rs      # ファイル探索
├── meta.rs      # メタデータ抽出
├── coarse.rs    # 高速ハッシュ
├── phash.rs     # 知覚ハッシュ
├── chroma.rs    # 音声フィンガープリント
├── db.rs        # データベース操作
├── cluster.rs   # 重複検出
├── interact.rs  # 対話UI
├── progress.rs  # 進捗表示
└── util.rs      # ユーティリティ
```

### ビルド方法
```bash
# 開発ビルド
cargo build

# リリースビルド
cargo build --release

# テスト実行
cargo test

# ドキュメント生成
cargo doc --open
```

## 📄 ライセンス

MIT OR Apache-2.0

## 🤝 コントリビューション

Issues や Pull Requests を歓迎します！

### 報告・改善要望
1. 既存のissueを確認
2. 新しいissueを作成
3. 可能な限り詳細な情報を提供

### 開発参加
1. このリポジトリをfork
2. feature branchを作成
3. 変更をcommit
4. pull requestを作成

## 📞 サポート

- **Issue**: バグ報告・機能要望
- **Discussion**: 使用方法に関する質問
- **Documentation**: このREADMEとコード内ドキュメント

---

**⚠️ 免責事項**: このツールは細心の注意を払って設計されていますが、重要なファイルは必ずバックアップを取ってから使用してください。開発者は一切の責任を負いません。