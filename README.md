# GitHub Issues Triage

GitHub Issues の最新10件を取得し、Jev で重要度順に並べる CLI ツールです。

## Components

```text
               ┌─────────────────────┐
               │     GitHub App      │
               │                     │
               │ Client ID           │
               │ Permissions         │
               │ Installation        │
               └──────────┬──────────┘
                          │ enables
                          ▼
               ┌─────────────────────┐
               │     Device Flow     │
               │                     │
               │ User authorization  │
               │ Access token        │
               │ Refresh token       │
               └──────────┬──────────┘
                          │ access token
           ┌──────────────┴──────────────┐
           ▼                             ▼
┌─────────────────────┐       ┌─────────────────────┐
│   GitHub REST API   │       │  GitHub GraphQL API │
│                     │       │                     │
│ Issues              │       │ Issues              │
│ Repositories        │       │ Repositories        │
│ Users               │       │ Projects            │
└──────────┬──────────┘       └──────────┬──────────┘
           └──────────────┬──────────────┘
                          │ Issue data
                          ▼
               ┌─────────────────────┐
               │  gh-issues-triage   │
               └──────────┬──────────┘
                          │ Issue content
                          ▼
               ┌─────────────────────┐
               │        Jev          │
               │                     │
               │ Importance scoring  │
               └──────────┬──────────┘
                          │ Scores
                          ▼
               ┌─────────────────────┐
               │    Ranked Issues    │
               └─────────────────────┘
```

private repository を使う場合は、GitHub App を対象 repository に install し、App の `Issues: Read` 権限を設定してください。

## Next Issue (JSON)

```shell
gh-issues-triage next owner/repo --format json
# リポジトリから実行する場合
cargo run -- next owner/repo --format json
```

最新の open Issue 最大10件（pull request を除外）から、仕様整理に着手可能（Ready が `Yes`）で重要度が最も高い1件を返します。同点の場合は GitHub の取得順（作成日時の新しい順）を維持します。古い Issue を含む全件からの選択ではありません。

標準出力は、末尾に改行を付けた単一行の JSON です。

```json
{"repository":"owner/repo","number":42,"title":"保存処理の修正","body":"再現条件と完了条件。","score":3.6,"readiness":"Yes"}
```

本文なしは空文字で返します。重要度は丸めません。該当する Issue がない場合は `null` を返し、終了コード `0` で終了します。入力不正は終了コード `2`、認証・取得・判定・出力の失敗は終了コード `1` です。失敗を `null` へ置き換えません。

認証方式と環境変数は既存の一覧コマンドと共通です。認証案内とエラーは標準エラー出力へ出します。JSON はパイプやリダイレクトで取得できます。端末幅やプロジェクトルートの設定は不要です。ローカルブランチの有無は選択に影響せず、Issue やブランチは変更しません。

## Environment

- Rust
- `reqwest` crate
- `GITHUB_CLIENT_ID`: GitHub App client ID（token 更新と Device Flow 用）
- `TYPESAFE_API_KEY`: TypeSafe Jev API key（Issue triage 用）

GitHub 公式 REST API を使って認証とデータの取得を行います。

## How to Use

```shell
export GITHUB_CLIENT_ID='<GitHub App client ID>'
export TYPESAFE_API_KEY='<TypeSafe API key>'
gh-issues-triage owner/repo
# リポジトリから実行する場合
cargo run -- owner/repo
```

認証情報がない場合は Device Flow の確認 URL とコードを表示します。認証情報は macOS Keychain または Linux Secret Service に保存されます。保存済み access token が失効した場合は refresh token で自動更新し、refresh token がないか失効している場合だけ Device Flow を開始します。PAT と平文ファイルは使用しません。

repository 指定では、GitHub の open Issue を新しい順に最大10件取得し、pull request を除外します。全 Issue を1回の TypeSafe Jev リクエストで重要度判定、ブランチ名の分類、着手可能度の判定を行い、端末幅に合わせた Unicode 対応テーブルで表示します。

`Ready` 列は、重要度とは独立して `brain-dump-docs` による仕様整理への着手可能度を表示します。具体的な振る舞いや問題が分かり、要求の骨格を創作せず整理できれば `Yes` です。話題や一般的な希望だけの場合や、基本動作・検出対象・違反条件などの機能の核心が不明な場合は `Needs information` です。質問を列挙できるだけでは `Yes` にしません。

機能の核心が分かる場合、出力項目、完了条件、再現手順、原因、実装方法、実現可能性などの詳細は Open Questions に残せます。`Needs investigation` は、仕様整理を開始する前に事実調査が不可欠な場合です。情報不足と調査必要が重なる場合は情報不足を優先します。実装への着手可否は判定せず、Ready は重要度順やブランチ作成操作を変更しません。

Issue 番号の右隣には、対象ローカルリポジトリのブランチ状態を表示します。`🌿` はブランチあり、空欄はブランチなし、`?` は確認不能です。`refactor` / `fix` / `feat` / `chore` / `docs` のいずれかの prefix を持つ `issue-<number>` を照合し、リモート上だけのブランチや独自命名のブランチは対象に含めません。root 未設定などで確認できない場合も、理由を示して一覧を表示します。この画面で作成に成功した後は、一覧へ戻る際に状態を更新します。

ブランチ作成を使う場合は、ローカルプロジェクト群の親ディレクトリを先に設定します。

```shell
cargo run -- config set-root /absolute/path/to/projects
cargo run -- owner/repo
```

`owner/repo` のローカル対象は `<root>/repo` です。Issue 本文下のボタンをクリックするか、`1`〜`9`・`0`（10件目）でブランチを作成します。

Issue 一覧の操作は次のとおりです。

| 操作 | 動作 |
| --- | --- |
| マウスオーバー | ポインターのある Issue を仮選択。対象外へ移動すると解除 |
| `j` / 下矢印、`k` / 上矢印 | 次／前の Issue を仮選択し、必要なら自動スクロール。初回は先頭の表示 Issue を選択 |
| PageUp / PageDown | 画面単位でスクロールし、長い本文を閲覧 |
| `q` | 終了 |

仮選択した Issue は、タイトル・本文・ボタンを含むセル全体を ANSI 16色の薄い青（bright blue）背景・黒文字でハイライトします。枠線の色は変えません。マウス移動と仮選択キーの最後の操作に合わせて選択を切り替え、仮選択だけではブランチを作成しません。ブランチ作成結果の画面では、`j`/`k`・上下矢印は1行ずつスクロールします。

ブランチ名は `<prefix>/issue-<number>` で、対象リポジトリの `main` を起点にします。作成時に対象リポジトリを新しいブランチへ切り替えます。CLI の作業ディレクトリ変更と push は行いません。root は `XDG_CONFIG_HOME/gh-issues-triage/config.json`、未設定時は `~/.config/gh-issues-triage/config.json` に保存します。

GitHub または TypeSafe の応答、認証情報、端末幅の取得に失敗した場合は、途中結果を表示せず非ゼロで終了します。実 GitHub App の認証と TypeSafe API の確認は、自動テストとは別に行います。一覧のハイライトと操作は、2026-10-01 にユーザーが実端末で動作確認済みです。

![GitHub Issues Triage](./docs/images/gh-issues-triage-screenshot.png)
