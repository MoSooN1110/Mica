# 07. 設定とCLI仕様

対象: 設定ファイル、キーマップ、CLI起動仕様。

---

## 1. 設定ファイル

### 1.1 配置

```text
ユーザー設定:        ~/.config/mica/config.toml   (XDG_CONFIG_HOME準拠)
ワークスペース設定:  <workspace>/.mica/config.toml
テーマ:              ~/.config/mica/themes/<name>.toml
```

マージ順: 組み込み既定値 → ユーザー設定 → ワークスペース設定(後勝ち)。

ワークスペース設定は信頼境界を越えるため、コマンド実行を伴う項目(シェル、LSPサーバーコマンド、タスク定義)は、利用者の明示的な承認なしに有効化しない。

### 1.2 設定項目(1.0)

```toml
[editor]
tab_width = 4
insert_spaces = true
line_numbers = true
active_line_highlight = true
mouse = true
auto_reload_unmodified = true
large_file_threshold_mb = 10
word_wrap = false
trim_trailing_whitespace = false
insert_final_newline = false
auto_pairs = true
ambiguous_width_wide = false
format_on_save = false

[workspace]
show_hidden = false
follow_symlinks = false
respect_gitignore = true

[git]
enabled = true
auto_refresh = true
confirm_destructive_actions = true

[terminal]
shell = ""              # 空なら$SHELL
scrollback_lines = 10000

[diagnostics]
enabled = false
inline_messages = false
check_on_save = false

[lsp]
enabled = false
# 言語別定義は06_diagnostics_lsp.mdの[languages.*]

[ui]
theme = "mica-dark"     # 名前または~/.config/mica/themes/内のファイル名
locale = "auto"         # "auto" | "en" | "ja"(UIの表示言語。01_ui.md §7参照)
icon_mode = "unicode"   # "ascii" | "unicode" | "nerd_font"
sidebar_width = 30
bottom_panel_height = 12
high_contrast = false
reduced_decoration = false

[keymap]
"ctrl-p"        = "workspace.open_file"
"ctrl-shift-p"  = "command_palette.open"
"ctrl-s"        = "editor.save"
"ctrl-alt-f"    = "terminal.search"
"f1"            = "help.keybindings"
"alt-n"         = "notifications.history"
# キーマップはコマンドIDへの割り当て。既定は01_ui.md参照
```

### 1.3 設定エラーの扱い

- 設定エラーで起動不能にしない
- エラー箇所(ファイル、キー、行)を通知に表示する
- 不正な値は既定値へフォールバックする
- 未知の項目は警告する(タイポ検出)

### 1.4 反映

- `config.reload`でユーザー設定とワークスペース設定を再読込する
- 起動後は対象設定ファイルを監視し、作成・更新・置換時に同じ再読込経路を実行する
- 設定とキーマップの解析、および外部テーマの読込はUIスレッド外で行う
- 読込に失敗したテーマは現在のテーマを維持し、警告を通知する
- 実行中プロセスに関わる設定（ターミナルのシェル等）は次回起動するプロセスから反映する
- `config.open`は`<workspace>/.mica/config.toml`を開く。未作成時は、既存パスを上書きせずスキーマコメント付き雛形を生成する

---

## 2. CLI仕様

### 2.1 起動

```text
mica [OPTIONS] [PATH]
```

```bash
mica .                    # カレントをワークスペースとして開く
mica /path/to/project
mica src/main.rs          # 親をワークスペースに、ファイルを開く
mica src/main.rs:42       # 42行目へ
mica src/main.rs:42:8     # 42行8列へ
```

### 2.2 オプション

```text
--line <N>          開くファイルの行
--column <N>        開くファイルの列
--readonly          読み取り専用で開く
--no-mouse          マウス無効(端末側の選択コピーを使う場合)
--config <PATH>     設定ファイルの指定
--log-file <PATH>   ログ出力先
--log-level <LVL>   ログレベル
--locale <LOCALE>   UI表示言語("en" | "ja")。既定はシステムロケールから自動判定
--safe-mode         ユーザー設定・テーマ・LSPを読み込まず起動
--check-config      設定とキーマップを検証して終了(警告時は非0)
--version
--help
```

### 2.3 PATH解釈

- ディレクトリ: ワークスペースとして開く
- ファイル: 親ディレクトリをワークスペースとして開き、対象ファイルを表示
- `file:line` / `file:line:column`: 指定位置へ移動(`--line`/`--column`より優先度は低い。両方指定時はオプションが勝つ)
- 存在しないファイル: 親ディレクトリが存在すれば新規バッファとして開く
- PATH省略: カレントディレクトリをワークスペースとして開く

### 2.4 終了コード

- 0: 正常終了
- 非0: 起動失敗(不正な引数、パス不存在等)。エラーはstderrへ1行で出す(TUI初期化前に判定できるものはTUIを立ち上げない)
