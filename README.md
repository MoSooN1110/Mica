# Mica

Rust製のターミナルネイティブIDE。VS Code風の画面構成(Explorer / Source Control / 統合ターミナル / Problems)を軽量なTUIとして提供する。

動作対象はWSL2(Ubuntu)とネイティブUbuntu。SSH先での利用に加え、Herdr(コーディングエージェントとターミナルウィンドウのオーケストレータ)の1ウィンドウとしての動作を想定する。tmux等のターミナルマルチプレクサは対象外。

## ドキュメント

- 要求: [SPEC/USER_WANTED.md](SPEC/USER_WANTED.md)
- 仕様書(索引): [SPEC/SPEC.md](SPEC/SPEC.md)
- 開発規約(エージェント向け): [AGENTS.md](AGENTS.md)

## ステータス

M1〜M4の主要機能を実装済み。

- **コアエディタ(M1)**: UTF-8・grapheme対応バッファ、Undo/Redo、安全な置換保存、
  外部変更監視と競合保護、確認付きファイル操作、階層Explorer、
  プレビュー付きファジーファイル検索、バッファ検索、Unicode範囲選択、
  内部/OSC 52クリップボード、Tree-sitterハイライト(Rust / Markdown / TOML)、
  コマンドパレット、キーマップ、設定・CLI
- **Gitと検索(M2)**: 状態表示、ファイル/ハンク差分、stage / unstage / restore、
  commit、branch一覧・切り替え・作成、push / pull / fetch、ワークスペース全文検索、
  クラッシュ復旧ジャーナルとセッション復元
- **統合ターミナル(M3)**: 実PTY、VT解釈、リサイズ、スクロールバック、
  フルスクリーンTUI対応
- **診断とLSP(M4)**: cargo check / clippy とLSP診断の統合、Problemsパネル、
  エディタ下線・ガター・ツリーバッジ、最小LSPクライアント
  (diagnostics / hover / 定義ジャンプ / 補完。C/C++, Rust, Python, JSON, Markdown)

```bash
cargo run -- .
```

進捗の基準と最終的な受け入れ条件は`SPEC/SPEC.md`を正とする。

## 開発

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```
