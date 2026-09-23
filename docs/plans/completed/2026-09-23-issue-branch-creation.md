# Issue からのブランチ作成

## Status

- Phase: implemented_pending_manual_validation
- Approval: approved (Tasks 1–3; Verification はユーザー編集後の文言)
- Automated verification: Stop hook の `make check`（fmt、Clippy、35テスト）と `make build` が成功。未コミット Rust 関連差分をレビューし、未解決の指摘なし。`git diff --check` 成功。
- Manual verification: 実 TypeSafe API、実 GitHub アカウント、実端末でのクリック・キーボード操作・終了時復元は未実施。

## Goal

トリアージ結果から Issue を選び、ローカルの対象プロジェクトで `main` を起点に `<prefix>/issue-<number>` ブランチを作成できるようにする。

## Scope

- Jev による Issue ごとの5分類と応答検証
- Issue 番号を使ったブランチ名の組み立てと検証
- トリアージ結果上の明示的なブランチ作成操作
- ローカル対象ディレクトリの確認と Git ブランチ作成
- ローカル root の設定と保存
- 必要な自動検証と利用方法の更新

## Non-goals

- リモートへの push、pull request 作成、Issue の更新
- 対象ディレクトリの自動 clone、`main` 以外への起点変更
- 既存ブランチの上書き
- ブランチ作成後の checkout、CLI の作業ディレクトリ変更

## Required Behavior

- [追加要求](../../current/REQUIREMENTS.md#追加要求-issue-からのブランチ作成) と [追加仕様](../../current/SPECIFICATIONS.md#追加仕様-ブランチ作成) を満たす。
- 既存の認証、Issue 取得、重要度順、端末幅と Unicode 表示の契約を維持する。
- ブランチ名は Jev の分類と Issue 番号から作り、Issue タイトルの翻訳・要約は行わない。

## Tasks

### Task 1: Jev 分類とブランチ名の準備

- Status: implemented。5分類、欠落・不正回答、Issue 番号との対応を自動テストで確認。実 TypeSafe API は未確認。
- Objective: 各 Issue に有効な prefix と `<prefix>/issue-<number>` を対応付ける。
- Scope: Jev `Choice` question、応答検証、Issue 番号からのブランチ名組み立て。
- Acceptance Criteria: 5種の prefix のいずれかと、対象 Issue 番号を含むブランチ名が対応する。不正・欠落した分類結果は推測値で補わない。
- Verification: 5種の分類、欠落・不正な Jev 回答、Issue 番号とブランチ名の対応を確認。hook で自動発火する検証の回答に従うこと。実 TypeSafe API は別途手動確認。

### Task 2: root 設定と Git ブランチ作成

- Status: implemented。一時設定ファイルと一時 Git リポジトリで保存・再読込、`main` 起点、同名ブランチ拒否、リポジトリ不一致、現在ブランチ不変を自動テストで確認。
- Objective: 利用者が選んだ Issue のブランチだけを対象プロジェクトに作る。
- Scope: root 設定とファイル保存、対象ディレクトリ解決、Git リポジトリ・`main`・同名ブランチの確認、ブランチ作成。
- Acceptance Criteria: root 設定が次回起動後も残る。指定先の `main` を起点にブランチを作成し、現在のブランチと作業ディレクトリを変更しない。未設定・不在・不一致・同名ブランチ・Git 失敗時に上書き・代替作成しない。
- Verification: 一時設定ファイルと Git リポジトリで保存・再読み込み、起点、名前、失敗条件、現在のブランチ不変を確認。hook で自動発火する検証の回答に従うこと。

### Task 3: 端末のブランチ作成ボタン

- Status: implemented。本文下のボタン位置、クリック範囲、スクロール後の Issue 対応、キー入力、復元制御列を自動テストで確認。実端末の手動確認は未実施。
- Objective: 各 Issue の本文下のボタンから、選択した Issue のブランチを作る。
- Scope: ボタン描画、マウス・キーボード操作、Task 2 との接続、結果表示、端末状態の復元。
- Acceptance Criteria: クリックした Issue だけが対象となり、操作前には Git 変更がない。クリック後は作成結果または理由付きエラーを表示し、端末を正常に復元する。
- Verification: 表示位置と選択対象の動作確認、実端末でのクリック・キーボード操作・終了時復元、Task 2 の Git 境界テスト。hook で自動発火する検証の回答に従うこと。

## References

- [要求定義](../../current/REQUIREMENTS.md)
- [仕様](../../current/SPECIFICATIONS.md)
- [設計](../../current/DESIGN.md)
- [TypeSafe Choice](https://docs.typesafe.ai/primitives/choice)
