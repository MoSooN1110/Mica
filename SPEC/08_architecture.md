# 08. アーキテクチャ仕様 — 技術構成・モジュール・状態・コマンド層

対象: 技術スタック、モジュール構造、イベントフロー、状態管理、コマンド層。

---

## 1. 技術構成

```text
Rust
├── ratatui               TUI描画
├── crossterm             端末制御、キー・マウス入力
├── tokio                 非同期タスク、プロセス管理
├── ropey                 Ropeベースのテキストバッファ
├── tree-sitter           構文解析・ハイライト
├── portable-pty          統合ターミナルのPTY
├── alacritty_terminal/vte VT互換のターミナルエミュレーション
├── notify                ファイル監視
├── ignore                .gitignore準拠の走査
├── grep-searcher/regex   全文検索
├── nucleo-matcher        ファジーマッチ
├── unicode-width         端末表示幅
├── unicode-segmentation  grapheme処理
├── lsp-types             LSP型定義
├── serde + toml          設定・状態のシリアライズ
├── tracing               ログ
└── git CLI(+ 任意でgit2) Git操作(バックエンド抽象の背後)
```

方針:

- Gitバックエンドは抽象化し、`git` CLI(既定)と`git2`を切り替え可能にする([04_git.md](04_git.md))
- 高度なGit・VT・LSPの独自再実装をしない。実績あるcrateと外部ツールを使う
- 外部コマンドは引数配列で実行する。シェル文字列連結を禁止する
- これらの選定を置き換える場合は、理由をPR・変更サマリに明記する

## 2. モジュール構造

```text
src/
  main.rs                 # 引数解釈、初期化、panicフック
  app/
    mod.rs
    state.rs              # AppState(状態の集約)
    event.rs              # AppEvent定義
    event_loop.rs         # イベント集約とディスパッチ
    update.rs             # イベント→状態遷移
  command/
    mod.rs
    registry.rs           # コマンドID→実装の登録
    id.rs                 # コマンドID定数
    palette.rs            # コマンドパレット
  workspace/
    mod.rs
    tree.rs               # ファイルツリーモデル
    path.rs               # 正規化・境界検証
    fs_ops.rs             # 作成・削除・改名・移動
    watcher.rs            # ファイル監視
  buffer/
    mod.rs
    text_buffer.rs        # ropeとバッファ状態
    selection.rs          # 選択モデル(複数選択へ拡張可能)
    history.rs            # Undo/Redo
    persistence.rs        # 保存、置換書き込み、スワップ/ジャーナル
  editor/
    mod.rs
    view.rs               # ビュー(カーソル・スクロール)
    movement.rs           # grapheme/表示列を考慮した移動
    rendering.rs
    syntax.rs             # tree-sitter統合
  git/
    mod.rs
    backend.rs            # バックエンド抽象(CLI / git2)
    status.rs
    diff.rs
    commit.rs
    branch.rs
    sync.rs               # push / pull / fetch
  terminal/
    mod.rs
    manager.rs            # セッション管理
    session.rs            # PTYと子プロセス
    screen.rs             # VT状態→描画モデル
    input.rs              # キー→PTYシーケンス
  diagnostics/
    mod.rs
    model.rs              # Diagnostic型
    store.rs              # ソース別世代管理・重複排除
    parsers/              # cargo, clang, 汎用パーサ
  lsp/
    mod.rs
    client.rs
    transport.rs          # JSON-RPC framing
    manager.rs            # サーバーのライフサイクル
    servers.rs            # 言語別定義
  search/
    mod.rs
    files.rs              # ファジーファイル検索
    text.rs               # 全文検索
  ui/
    mod.rs
    layout.rs
    focus.rs
    theme.rs              # セマンティックトークン、256色近似
    components/           # activity_bar, sidebar, tabs, status_bar,
                          # bottom_panel, dialogs, palette, ...
  config/
    mod.rs
    settings.rs
    keymap.rs
  session/
    mod.rs                # セッション保存・復旧
    recovery.rs
  logging.rs
tests/
```

構造の変更は妨げないが、レイヤ境界(下記)を壊す変更はしない。

## 3. 依存方向

```text
UI → Command → ドメインサービス(buffer/workspace/git/…) → インフラ(fs/process/pty)
```

禁止する依存:

- ドメイン層(buffer, workspace, git, diagnostics, lsp, search)からratatui・crossterm・UIウィジェットへの依存
- UIイベントハンドラからドメイン処理の直接呼び出し(必ずコマンドまたはイベント経由)
- 描画コードからの状態変更・プロセス生成・ファイルI/O・Git実行・LSP通信

## 4. イベントフローと状態管理

単一方向のフローとする。

```text
Terminal Input / File Watcher / Git Task / Search Task /
LSP / PTY Output / Syntax Task / Timer / Resize / Shutdown
        ↓ (集約)
     AppEvent
        ↓
  Command / Update      … 状態遷移 or 非同期タスク発行
        ↓
     AppState
        ↓
      Render            … 状態の読み取りのみ
```

要件:

- アプリケーション全体で整合した単一の状態ツリー(`AppState`)を持つ。ただしサブシステムの状態は明確に分割し、全モジュールが自由に触れる神オブジェクトにしない
- 状態の変更はイベント処理(update)でのみ行う。描画から状態を変更しない
- 非同期タスクの結果は型付きイベントとして戻す。共有ミュータブル状態への直接書き込みで戻さない
- グローバルなミュータブルシングルトンを導入しない

## 5. バックグラウンド実行

以下はUI(描画・入力)スレッドで実行しない。

- ファイルツリー走査、ファイル監視
- 全文検索・ファジー検索のインデックス
- Git status / diff / commit / push / pull
- LSP通信、コンパイラ・リンター実行
- PTY I/O
- 大きなドキュメントの構文解析
- ファイルのロード・保存

すべての長時間タスクはキャンセル可能にする(新しい検索が古い検索を止める、等)。

## 6. 障害隔離

- LSPサーバーのクラッシュ、Git失敗、シェルの終了がエディタ本体を落とさない
- サブシステムの失敗はOutputパネル・ステータスバー・非モーダル通知で可視化する
- panic時は端末状態(raw mode、alternate screen、マウスキャプチャ)を必ず復元する(panicフック+RAIIガード)

## 7. コマンド層

すべての主要操作は一意な文字列IDを持つコマンドとして登録する。キーボード、マウス、コマンドパレットは同一のコマンド層を呼ぶ。

コマンドID例:

```text
file.new  file.new_directory  file.rename  file.move  file.delete
editor.save  editor.save_as  editor.close  editor.undo  editor.redo
editor.toggle_pin  editor.move_tab_left  editor.move_tab_right
workspace.open_file  workspace.search  workspace.refresh
git.open_panel  git.open_diff
git.stage_file  git.unstage_file  git.stage_hunk  git.unstage_hunk
git.restore_file  git.restore_hunk
git.commit  git.branch_switch  git.branch_create
git.push  git.pull  git.fetch
terminal.toggle  terminal.new_session  terminal.open_reference  terminal.search
diagnostics.open_problems
lsp.hover  lsp.goto_definition  lsp.completion
view.toggle_sidebar  view.explorer  view.source_control  view.search
command_palette.open
config.reload  config.open
help.keybindings
notifications.history
```

要件:

- コマンドはコンテキスト(フォーカス、選択対象)を引数として受け取り、UI実装に依存しない
- 実行不可のコマンドは理由とともに無効表示する(パレット・メニュー)
- コマンド層は将来の外部制御(`micactl`、Unix Domain Socket、JSON Linesイベント等)へ再利用できる形にする。外部制御API自体は1.0の対象外
- 将来のHerdr連携([SPEC.md](SPEC.md) §4.3.5)はこのコマンド境界を通して行う。Herdrは既にSocket API/CLI(`herdr pane report-metadata`等)を持つため、独自プロトコルの発明より先にその再利用を検討する。画面座標や疑似キー入力による制御を前提としない
