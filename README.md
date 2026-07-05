# Mica

Rust製のターミナルネイティブIDE。VS Code風の画面構成(Explorer / Source Control / 統合ターミナル / Problems)を軽量なTUIとして提供する。

動作対象はWSL2(Ubuntu)とネイティブUbuntu。SSH先での利用に加え、Herdr(コーディングエージェントとターミナルウィンドウのオーケストレータ)の1ウィンドウとしての動作を想定する。tmux等のターミナルマルチプレクサは対象外。

## ドキュメント

- 要求: [SPEC/USER_WANTED.md](SPEC/USER_WANTED.md)
- 仕様書(索引): [SPEC/SPEC.md](SPEC/SPEC.md)
- 開発規約(エージェント向け): [AGENTS.md](AGENTS.md)

## ステータス

M1(コアエディタ)を実装中。現在はRustプロジェクト、単方向イベント/コマンド層、
UTF-8・grapheme対応バッファ、Undo/Redo、安全な置換保存、ワークスペース境界検証、
非同期ファイル走査/ロード/保存、外部変更監視と競合保護、確認付きファイル操作、
階層Explorer、プレビュー付きファジーファイル検索、バッファ検索、Unicode範囲選択、
内部/OSC 52クリップボード、Rust用Tree-sitterハイライト、クラッシュ復旧ジャーナル、
セッション復元、タブ操作、設定・CLI、基本TUIを備える。

```bash
cargo run -- .
```

Source Control、全文検索、PTY、診断/LSP、構文ハイライト、セッション復旧などは未実装。
進捗の基準と最終的な受け入れ条件は`SPEC/SPEC.md`を正とする。
