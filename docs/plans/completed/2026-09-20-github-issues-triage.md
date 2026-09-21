# GitHub Issues Triage 初期実装

## Status

- Phase: implementation
- Approval: approved

## Goal

GitHub App の Device Flow で認証し、指定リポジトリの最新 open Issue 最大10件を Jev で重要度順に並べ、端末幅のテーブルとして表示する Rust CLI を実装する。

## Scope

- Rust CLI の初期構成
- GitHub App Device Flow と認証情報保存
- GitHub REST API による Issue 取得
- TypeSafe Jev `Score` による重要度算出
- Unicode 対応の端末テーブル表示
- 自動テスト、利用方法、必要設定の文書化

## Non-goals

- Issue の更新、ラベル付与、コメント、close
- pull request の triage
- 複数リポジトリの一括処理
- 対話 UI、TUI、Web UI
- ブラウザの自動起動
- オフラインキャッシュ、履歴、重要度の手動補正
- GitHub Enterprise Server 対応

## Required Behavior

- [REQUIREMENTS.md](../../current/REQUIREMENTS.md) を満たす。
- [SPECIFICATIONS.md](../../current/SPECIFICATIONS.md) の外部動作と失敗条件を満たす。
- [DESIGN.md](../../current/DESIGN.md) の責務と外部境界を維持する。
- [Decisions](#decisions) の承認内容を正本文書へ反映してから着手する。

## Tasks

### Task 1: Rust CLI の土台

- Status: completed
- Objective: 2つの実行形式を検証し、処理を選択できる最小のバイナリを作る。
- Scope: `Cargo.toml`、CLI 引数解析、`<owner>/<repo>` 解析、usage、エラーと終了コード。
- Acceptance Criteria: `login` と有効な repository 指定だけを受理し、不正入力は理由と usage を示して非ゼロ終了する。
- Verification: 引数と repository 解析の自動テスト、`cargo test`、`cargo clippy -- -D warnings`。
- Validation: 引数と repository 解析を含む自動テスト、`cargo fmt --check`、`cargo test`、`cargo clippy -- -D warnings` 成功。

### Task 2: GitHub Device Flow

- Status: implemented
- Objective: GitHub の認証を完了し、後続実行で token を取得できるようにする。
- Scope: device code 取得、案内表示、interval/slow_down/期限を守るポーリング、承認済み方式での token 保存と読み込み。
- Acceptance Criteria: 成功時だけ token を保存し、pending、slow down、拒否、期限切れ、HTTP エラーを区別して処理する。秘密情報を出力しない。
- Verification: 保存済み HTTP 応答による状態遷移テスト、保存境界テスト、実 GitHub App を使う手動 Device Flow 確認。
- Validation: `cargo fmt --check`、`cargo test`（13件）、`cargo clippy -- -D warnings` 成功。実 GitHub App と実資格情報ストアによる手動確認は未実施。

### Task 3: Issue 取得

- Status: implemented
- Objective: 指定 repository から対象となる最新 open Issue を最大10件得る。
- Scope: GitHub REST API、認証 header、ページング、PR 除外、必要フィールドの deserialize。
- Acceptance Criteria: PR が混在しても Issue を新しい順に最大10件返し、10件未満と0件を正常に扱う。認証・権限・不在・API 失敗を成功扱いしない。
- Verification: 複数ページ、PR 混在、10件未満、0件、HTTP エラーの境界テスト。
- Validation: GitHub 応答の複数ページ処理、PR 除外、10件上限、0件、HTTP/JSON エラー、request URL の純粋テストを実装し、Task 1–2 回帰テストを保持した `cargo test` 23件、`cargo fmt --check`、`cargo clippy -- -D warnings` 成功。実 GitHub API は未実施。

### Task 4: Jev triage

- Status: implemented
- Objective: 全 Issue の重要度を1回の Jev request で算出し、降順に並べる。
- Scope: structured state、Issue ごとの Score question、応答検証、安定ソート、上限付き再試行。
- Acceptance Criteria: 各 Issue に検証済みの Jev `score` が1つ対応し、同順位では取得順を維持する。不完全・不正な応答を推測値で補わない。
- Verification: request shape、Score の端点、欠落・型違い・範囲外、同順位、429/529 と再試行上限の自動テスト。代表 Issue を使う実 API の手動確認。
- Validation: request shape、空IssueのAPIキー不要分岐、Score 応答の欠落/余分/type/範囲/確率加重score/legend/probabilities、安定ソート、429/529 の再試行判定・遅延上限、Task 1–2 の認証状態遷移回帰テストを実装し、`cargo test` 23件、`cargo fmt --check`、`cargo clippy -- -D warnings` 成功。実 TypeSafe API は未実施。

### Task 5: テーブル表示と結合

- Status: implemented
- Objective: triage 結果を仕様どおり端末幅のテーブルへ描画する。
- Scope: 端末幅取得、固定列と可変列の配分、Unicode 折り返し、複数行セル、罫線、全処理の接続、README 更新。
- Acceptance Criteria: 各行が端末幅と一致し、セルが上・左寄せで、内側 `│` の左右に半角空白があり、日本語と絵文字が列を越えない。0件は空テーブルとなる。
- Verification: 複数の端末幅、日本語、絵文字、改行、長文、0件、幅不足の snapshot または文字列比較テスト、`cargo fmt --check`、`cargo test`、`cargo clippy -- -D warnings`、実端末での目視確認。
- Validation: Unix `ioctl` の端末幅取得、複数幅、Unicode 表示幅折り返し、改行、長文、0件、幅不足、全行幅一致の文字列テストを実装し、Task 1–2 回帰テストを保持した `cargo test` 23件、`cargo fmt --check`、`cargo clippy -- -D warnings` 成功。実端末の目視確認は未実施。

## Decisions

1. 対象は GitHub.com とし、macOS と Linux に対応する。
2. GitHub App client ID は `GITHUB_CLIENT_ID` 環境変数から取得する。client secret は使用しない。
3. GitHub App user access token は OS の資格情報ストアへ保存する。PAT と平文ファイルへの fallback は使用しない。
4. GitHub App とユーザーの権限でアクセスできる private repository を対象に含める。
5. 「最新」は `created_at` の降順とする。
6. Jev が返す検証済みの `score` を変換せず重要度として使用する。

## References

- [要求定義](../../current/REQUIREMENTS.md)
- [仕様](../../current/SPECIFICATIONS.md)
- [設計](../../current/DESIGN.md)
- [GitHub Device Flow](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow)
- [GitHub REST API: List repository issues](https://docs.github.com/en/rest/issues/issues#list-repository-issues)
- [TypeSafe Score](https://docs.typesafe.ai/primitives/score)
- [TypeSafe HTTP API](https://docs.typesafe.ai/api)
