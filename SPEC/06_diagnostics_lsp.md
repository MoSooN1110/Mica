# 06. 診断とLSP仕様

対象: 診断モデル、診断ソース(LSP・コンパイラ・リンター)、Problemsパネル、エディタ表示、最小LSPクライアント、言語別設定。

---

## 1. 診断モデル

すべての診断ソースは単一のモデルへ正規化する。

```rust
pub struct Diagnostic {
    pub file: PathBuf,
    pub range: TextRange,          // バッファ内の開始・終了位置
    pub severity: DiagnosticSeverity, // Error | Warning | Information | Hint
    pub message: String,
    pub source: DiagnosticSource,  // Lsp | Compiler | Linter | Task
    pub code: Option<String>,      // E0308, clippy::xxx など
}
```

### 1.1 ソース

- LSPサーバー(`textDocument/publishDiagnostics`)
- コンパイラ(例: `cargo check --message-format=json`)
- リンター(例: `clippy`)
- タスクランナーの出力パーサ(汎用)

構造化出力(JSON)が利用できるツールでは、人間向けテキストのパースより構造化出力を優先する。

### 1.2 重複排除

複数ソースが同等の診断を報告した場合、以下のキーで同一視する。

- 正規化済みファイルパス、開始位置、終了位置、正規化済みメッセージ、severity

重複時の優先順位: コンパイラ → LSP → リンター → 汎用パーサ。

### 1.3 ライフサイクル

- ソースごとに診断の世代を管理し、新しい結果が来たらそのソースの旧診断を置き換える
- ファイル保存・変更で古い位置情報が実態とずれる場合、バッファ編集に追従して位置をシフトするか、stale表示とする
- ソースの失敗(LSPクラッシュ等)でそのソースの診断をクリアし、失敗自体をOutputへ記録する

## 2. 表示

診断は以下のすべてに表示する。

- エディタ内の下線(可能なら波線、端末非対応時は色付き下線・反転等の代替)
- エディタガターのマーカー(severity記号)
- ファイルツリーのバッジ(ファイル・フォルダ単位のエラー/警告有無)
- Problemsパネル(下部パネル)
- ステータスバーのエラー・警告件数

### 2.1 Problemsパネル

- ファイル単位でグループ化した一覧(severity記号+色、メッセージ、ソース、code、行:列)
- 項目選択で該当位置へジャンプ
- severityでのフィルタ
- 件数が多くても逐次描画し、UIを停止させない

## 3. 最小LSPクライアント

### 3.1 スコープ

LSPは以下の機能に限定する。フルのVS Code互換クライアントを目指さない。

- diagnostics(受信)
- hover
- 定義ジャンプ(definition)
- 補完(completion。基本的な項目表示と挿入。スニペット展開は非対応でよい)

### 3.2 必須クライアントメッセージ

```text
initialize / initialized / shutdown / exit
textDocument/didOpen
textDocument/didChange
textDocument/didSave
textDocument/didClose
textDocument/hover
textDocument/definition
textDocument/completion
```

### 3.3 サーバーからの受信

- `textDocument/publishDiagnostics`を処理する
- progress・ログ系メッセージはOutputパネルへ記録してよい
- 未知の通知・リクエストでクラッシュしない(通知は無視、リクエストは適切なエラー応答)

### 3.4 実装方針

- 同期は最初はfull document syncで実装し、正しさを確立してからincrementalを検討する
- `initialize`では実装済みのcapabilityのみを申告する
- サーバープロセスはUIスレッド外で管理する。クラッシュはエディタを巻き込まず、再起動(回数制限付き)と失敗の可視化を行う
- 位置情報はLSPのUTF-16オフセットとバッファ内部表現の変換を正しく行う(Unicodeテスト対象)
- Mica終了時にサーバーへ`shutdown`/`exit`を送り、プロセスを確実に回収する

## 4. 言語別設定

既定のサーバー定義(利用者が設定で上書き可能):

```toml
[languages.c_cpp]
extensions = ["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"]
command = "clangd"
args = []
root_markers = ["compile_commands.json", "CMakeLists.txt", ".git"]

[languages.rust]
extensions = ["rs"]
command = "rust-analyzer"
args = []
root_markers = ["Cargo.toml", ".git"]

[languages.python]
extensions = ["py", "pyi"]
command = "pyright-langserver"
args = ["--stdio"]
root_markers = ["pyproject.toml", "setup.py", "requirements.txt", ".git"]

[languages.json]
extensions = ["json", "jsonc"]
command = "vscode-json-language-server"
args = ["--stdio"]
root_markers = [".git"]

[languages.markdown]
extensions = ["md", "markdown"]
command = "marksman"
args = ["server"]
root_markers = [".git"]
```

要件:

- 実行ファイル名・引数は設定で変更可能
- サーバーが見つからない場合は明確で致命的でない警告を1回だけ出す(ファイルを開くたびに繰り返さない)
- ルート検出は`root_markers`を優先し、なければワークスペースルート
- サーバーの起動はそのファイルを開いたときの遅延起動とする
