# GitHub Issues Triage 仕様

## 状態

この文書は承認済み仕様である。

## CLI

```text
gh-issues-triage <owner>/<repo>
```

- 引数なし、未知のサブコマンド、または `<owner>/<repo>` として解釈できない値は usage を標準エラー出力へ表示し、非ゼロで終了する。
- `owner` と `repo` は空文字を許可しない。余分なパス要素を許可しない。
- 成功時は終了コード `0`、入力・認証・通信・応答・保存・表示の失敗時は非ゼロとする。

## 認証

1. 保存済み access token が有効なら、Issue の取得にそのまま使用する。旧版が保存した access token 単体の値も読み取る。
2. access token が失効した場合、保存済み refresh token があれば `POST https://github.com/login/oauth/access_token` に `client_id`、`grant_type=refresh_token`、`refresh_token` を送り、token を更新する。Device Flow で発行した token のため client secret は使用しない。
3. 更新後の access token と refresh token を組として OS の資格情報ストアへ保存し、新しい access token で Issue 取得を再試行する。
4. 認証情報が未保存、refresh token がない、または GitHub が `bad_refresh_token` を返した場合だけ Device Flow を開始する。資格情報ストアや通信の失敗、その他の更新エラー、repository の権限不足は理由を示して失敗終了する。
5. Device Flow では、GitHub App の client ID を `GITHUB_CLIENT_ID` 環境変数から読み込む。未設定または空の場合は失敗する。
6. `POST https://github.com/login/device/code` で `device_code`、`user_code`、`verification_uri`、`expires_in`、`interval` を取得する。
7. `verification_uri` と `user_code` を表示する。ブラウザの自動起動は行わない。
8. GitHub が返した `interval` 以上の間隔で `POST https://github.com/login/oauth/access_token` をポーリングする。
9. `authorization_pending` は待機を継続し、`slow_down` は GitHub の指示どおり待機間隔を延長する。
10. 成功時だけ access token と、発行された場合の refresh token を資格情報ストアへ保存し、Issue 取得を続ける。拒否、期限切れ、不正な client ID、Device Flow 無効化、通信失敗は理由を示して失敗終了する。

`GITHUB_CLIENT_ID` は token 更新または Device Flow が必要になったときに読み込む。

## `<owner>/<repo>`

### Issue の取得

- GitHub REST API の `GET /repos/{owner}/{repo}/issues` を使用する。
- `state=open`、`sort=created`、`direction=desc` とする。
- REST API が返す `pull_request` フィールド付き項目を除外する。
- 除外後に10件へ達するまで必要なページを読み、リポジトリに存在する open Issue が10件未満なら存在する件数だけを対象とする。
- triage に使用する GitHub の値は `number`、`title`、`body` とする。`body: null` は本文なしとして扱う。

「最新」は `created_at` の降順とする。

### Jev による重要度

- TypeSafe HTTP API `POST https://api.typesafe.ai/v1/systemone` を `Authorization: Bearer <API_KEY>` 付きで使用する。
- model は追従用エイリアス `jev-latest` とする。
- 10件を1つの構造化 `state` にまとめ、Issue ごとに独立した `Score` question を1つ作る。
- 各 question の instructions は、対象 Issue の `number`、`title`、`body` のパスと、重要度を判定することを明記する。question ID 自体には意味を依存させない。
- Score は次の5段階を低い順に使用する。

| level | 基準 |
| ---: | --- |
| 0 | 対応不要、情報提供のみ、または影響が確認できない |
| 1 | 影響が小さく、容易な回避策がある |
| 2 | 通常機能に影響するが、業務を止めない |
| 3 | 主要機能を妨げる、広い利用者に影響する、または回避が困難 |
| 4 | データ損失、セキュリティ、サービス停止など直ちに対応すべき影響 |

- API が返す確率加重 `score` を検証し、変換せず重要度として使用する。
- `confidence` は順位計算に使わず、初期版では表示しない。
- 一部でも回答が欠ける、不正な型である、または TypeSafe が失敗した場合は、推測値で補わずコマンド全体を失敗させる。
- `429` と `529` は上限付きの指数バックオフで再試行し、上限到達後は失敗させる。具体的な回数とタイムアウトは実装時にテスト可能な定数として決める。

### 並べ替え

- 重要度を降順に安定ソートする。
- 同じ重要度では GitHub から取得した順序を維持する。

## テーブル

```text
┌────────┬──────┬────────────────────────────────────┐
│ 重要度 │ 番号 │ Issues                             │
├────────┼──────┼────────────────────────────────────┤
│ 3.6    │ #42  │ Issue のタイトル                   │
│        │      │ Issue の内容                       │
└────────┴──────┴────────────────────────────────────┘
```

- 出力先が端末であり幅を取得できる場合、各行の表示幅をその端末幅と一致させる。
- 固定列は内容と左右 padding が収まる最小幅とし、残りを `Issues` 列へ割り当てる。
- `Issues` 列ではタイトル、その直後の行から本文を表示する。入力中の改行を保持し、各行を列幅で折り返す。
- 折り返しは Unicode の端末表示幅を基準にし、セル境界を越えない。
- Issue ごとに中罫線を置く。
- 端末幅を取得できない場合、または3列を成立させる幅がない場合は理由を示して失敗する。根拠のない既定幅へ置き換えない。
- 対象 Issue が0件の場合はヘッダーだけを持つ空テーブルを表示して成功終了する。

## 設定と秘密情報

- `TYPESAFE_API_KEY` は環境変数から取得し、永続保存しない。
- GitHub App client ID は `GITHUB_CLIENT_ID` 環境変数から取得する。client secret と PAT は使用しない。
- GitHub App user access token と、発行された場合の refresh token はサービス名 `gh-issues-triage`、アカウント名 `github.com` で OS の資格情報ストアへ保存する。
- macOS では Keychain、Linux では Secret Service を使用する。利用不能時は失敗し、平文ファイルへ fallback しない。
- 秘密情報をエラー本文へ含めない。

## 対象

- サービスは GitHub.com に限定する。
- OS は macOS と Linux に対応する。
- GitHub App とユーザーの権限でアクセスできる public/private repository を対象とする。

## 追加仕様: ブランチ作成

- 既存の重要度 `Score` 判定に加え、同じ Issue の `number`、`title`、`body` を参照する独立した Jev `Choice` question で `refactor` / `fix` / `feat` / `chore` / `docs` の一つを選ぶ。既存の最大10件・重要度順は維持する。
- `Choice` の回答は質問数、型、選択肢を検証する。不完全・不正な回答に既定の prefix を当てず、トリアージを失敗させる。
- Issue 番号を十進数で表し、`<prefix>/issue-<number>` をブランチ名とする。Issue タイトルの翻訳・要約や追加の生成 API は使用しない。
- `gh-issues-triage config set-root <directory>` で既存の絶対パスを root として保存する。設定先は `XDG_CONFIG_HOME/gh-issues-triage/config.json`、未設定時は `~/.config/gh-issues-triage/config.json` とする。設定値は公開パスであり token や API key は保存しない。
- CLI 引数 `<owner>/<repo>` の `repo` をローカルディレクトリ名とし、`root/repo` を対象にする。root 未設定・候補パス不在の場合はブランチ作成を拒否する。候補パスが指定されたリポジトリに対応するかを検証してから Git 操作を行う。
- ブランチの起点は対象ローカルリポジトリの `main`。`main` がない場合、別の ref を代用しない。同名ブランチがある場合、上書きしない。
- 各 Issue の本文の下にブランチ名とマウスでクリックできる作成ボタンを表示する。クリック後にブランチを作成する。端末はクリックを受け取る間、対話状態を維持する。キーボードでも同じ操作を選べるようにする。
- 作成時に対象リポジトリを新しいブランチへ切り替え、CLI の作業ディレクトリは変更しない。対象リポジトリを明示して Git 操作を実行する。クリック前にブランチを作成しない。
- ディレクトリ・Git・Jev のエラーは理由を表示し、成功表示しない。`repo` をシェル文字列へ直接埋め込まない。
