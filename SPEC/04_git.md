# 04. Git仕様 — 状態・差分・stage・commit・branch・同期

対象: Source Controlビュー、Git状態表示、差分表示、stage/unstage/restore、commit、branch操作、push/pull/fetch。

---

## 1. 原則

- 用語はGit標準に従う: Working Tree Changes / Staged Changes / Untracked / Conflicted
- Micaは「事実として確認できるGitの状態と差分」だけを扱う。変更が人間によるものかエージェントによるものかを推測・表示しない
- 破壊的操作(restore/discard、ブランチ削除等)は必ず確認を挟む。黙って実行しない
- Git操作中もUIを停止させない。状態取得・差分生成はキャンセル可能なバックグラウンドタスクとする

## 2. バックエンド

- Gitバックエンドは抽象化し、`git` CLIと`git2`(libgit2)を切り替え可能にする
- 既定は`git` CLI(挙動の互換性を優先。フック・credential helper・configがそのまま効く)
- `git2`は読み取り系(status、diff)の高速化に限定して利用してよい
- 外部コマンドは必ず引数配列で実行する。シェル文字列連結を禁止する
- 失敗時は実行コマンド、終了コード、stderrを保持し、Outputパネルで確認できるようにする

## 3. Git状態表示

### 3.1 必須表示

- 現在のブランチ(detached HEADの明示を含む)— ステータスバーとSource Controlビュー
- Staged / Working Tree(unstaged)/ Untracked / Conflictedのファイル一覧(セクション分け)
- ファイルごとの状態記号と色(記号+色の併用)
- 変更数の概要(ステータスバー)
- ahead / behind(upstreamとのコミット差)
- ignoredファイルは既定で非表示

### 3.2 更新契機

- 起動時、ファイル保存時、ファイル監視イベント時、Git操作完了時、手動更新時
- ターミナルでのコマンド実行後(プロンプト検知は行わず、監視イベント+デバウンスで拾う)
- 一定時間内のイベントはデバウンスする

## 4. 差分表示

### 4.1 必須機能

- working tree ↔ index、index ↔ HEADの差分
- ファイル単位・ハンク単位の差分表示(インライン差分ビュー)
- サイドバイサイド表示は任意(1.0で必須としない)
- 追加・変更・削除行の色分け+記号
- エディタ左端ガターの差分マーカー(追加・変更・削除)
- 差分から該当行へのジャンプ
- 大きな差分でもUIを停止させない。差分生成はキャンセル可能
- バイナリファイルは「バイナリ」として明示し、テキスト差分を試みない

### 4.2 差分操作

- ファイルのstage / unstage
- ハンクのstage / unstage
- ファイル変更のrestore(discard)
- ハンク変更のrestore(discard)
- restore系は破壊的操作として確認必須

## 5. Commit

必須機能:

- staged変更の一覧確認
- コミットメッセージ入力(複数行対応)
- 空メッセージの拒否
- commit実行、成功・失敗の表示
- Gitフック(pre-commit等)の失敗をstderr付きで表示する。フックを迂回しない(`--no-verify`は明示操作としてのみ将来検討)
- commit後の状態更新
- amendは1.0では対象外(将来拡張)

## 6. Branch

必須機能:

- ブランチ一覧(ローカル。リモート追跡は表示のみ)
- ブランチ切り替え(checkout / switch)
- 新規ブランチ作成(現在HEADから)
- 未保存バッファ・未コミット変更がある状態での切り替えは、Gitの失敗をそのまま表示し、強制しない
- 切り替え後はワークスペース全体を更新する(ツリー・バッファ・状態。未保存変更の保護は[03_workspace.md](03_workspace.md)に従う)
- ブランチ削除は1.0で任意。実装する場合は破壊的操作として確認必須

## 7. Push / Pull / Fetch

必須機能:

- push(現在のブランチをupstreamへ。upstream未設定時は`--set-upstream`を提案)
- pull(既定はff-only。ffできない場合は失敗として表示し、rebase/mergeの解決はターミナルへ誘導する)
- fetch
- 実行中の進捗表示と完了・失敗の通知
- 認証はGitの標準機構(ssh-agent、credential helper)に委ねる。Micaはパスワード・トークンを保存しない
- 対話的な認証プロンプトが必要な場合は失敗として検出し、ターミナルでの実行を案内する
- force pushはUIから提供しない(将来提供する場合も確認必須)

## 8. コンフリクト

1.0では解決UIを持たない。ただし:

- Conflicted状態のファイルを一覧・明示する
- コンフリクトマーカーを含むファイルの編集・保存は通常どおり可能
- 解決(`git add`)はstage操作として実行できる

## 9. Gitリポジトリがない場合

- ワークスペースが非Gitでも全機能(編集・検索・ターミナル・診断)が動作する
- Source Controlビューは「リポジトリなし」を表示する。`git init`の実行は1.0で任意
