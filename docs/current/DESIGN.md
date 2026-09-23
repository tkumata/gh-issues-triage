# GitHub Issues Triage 設計

## 方針

- 単一の Rust バイナリとして実装する。
- GitHub と TypeSafe には `reqwest` で直接 HTTP リクエストを送り、初期版のための独自 SDK や汎用 API 層は作らない。
- 外部 API、認証情報保存、端末表示の境界だけを分離し、単一実装の trait、factory、将来用設定は作らない。
- 処理途中の Issue 一覧や推測した重要度は成功結果として表示しない。

## 処理フロー

### Triage

```text
CLI 引数検証
 └─ 認証情報の取得
     ├─ 有効な access token はそのまま使用
     ├─ 失効時は refresh token で更新して保存
     └─ 未保存・refresh token なし/失効時は Device Flow で認証して保存
          ↓
     GitHub REST API から最新 open Issue を最大10件取得
      └─ 全 Issue を1回の Jev リクエストで Score 化
          └─ Score を検証して安定ソート
              └─ 端末幅に合わせてテーブルを描画
```

## 責務

| 責務 | 内容 |
| --- | --- |
| CLI | `<owner>/<repo>` の検証、usage、終了コード |
| GitHub authentication | 保存済み token の利用、refresh、必要時の Device Flow、GitHub エラーの解釈 |
| credential storage | OS の資格情報ストアを使った GitHub App user access token と refresh token の保存と読み込み |
| GitHub issues | REST API ページング、pull request 除外、最大10件への制限 |
| triage | Jev request の構築、Score 応答検証、安定ソート |
| presentation | Unicode 表示幅に基づく折り返し、列幅配分、罫線描画 |

実装時のファイル分割は責務が読みにくくならない最小単位とし、責務ごとのファイル作成を必須にしない。

## 主要データ

```text
RepositoryRef { owner, repo }
Issue { number, title, body, source_order }
RankedIssue { issue, score }
```

- 外部 API の応答型は必要なフィールドだけを deserialize する。
- `source_order` は同順位時の順序保持に使う。
- Jev の `score`、`confidence`、`probabilities` は応答検証に使用し、検証済みの `score` を変換せず表示と並べ替えに使う。

## 外部境界

### GitHub

- GitHub App は Device Flow を有効にする。
- access token 失効時は refresh token で更新する。`bad_refresh_token` は Device Flow に進み、その他の更新エラーは失敗として扱う。
- Device Flow の `interval`、`expires_in`、`slow_down` を GitHub の応答どおり扱う。
- Issues REST endpoint は pull request も返すため、`pull_request` の有無で除外する。
- HTTP status と GitHub の error body を、秘密情報を除いて利用者向けエラーへ変換する。

### 認証情報保存

- GitHub App client ID は `GITHUB_CLIENT_ID` 環境変数から取得する。
- GitHub App user access token と、発行された場合の refresh token はサービス名 `gh-issues-triage`、アカウント名 `github.com` で OS の資格情報ストアへ保存する。
- 旧版が保存した access token 単体の値も読み取れるようにする。
- macOS Keychain または Linux Secret Service が利用不能な場合は失敗し、平文ファイルへ fallback しない。

### TypeSafe

- Rust SDK は前提にせず、公式 HTTP endpoint を使用する。
- structured state と Issue ごとの Score question を1リクエストにまとめる。
- question の5段階 criterion はアプリ側の仕様として固定し、返却された Score を独自変換しない。
- 返却された answer の件数、type、有限数、許容範囲を検証してから使用する。

### 端末

- 端末幅の取得と Unicode 表示幅の計算は標準ライブラリで不足する境界であり、実装時に小さな既存 crate を選定してよい。
- テーブル描画そのものは要求が限定されるため、汎用 UI 層を作らない。

## エラー方針

- 利用者が直せる内容を先頭に示し、HTTP 応答や内部エラーの連鎖を保持する。
- timeout、レート制限、認証拒否、期限切れ、権限不足、repository 不在、TypeSafe 応答不正、端末幅不足を区別する。
- 認証前、保存完了前、全 Score 検証前には成功を報告しない。

## 検証境界

- 単体テスト: 引数解析、repository 解析、Score 検証、安定ソート、Unicode 折り返し、列幅と罫線。
- HTTP 境界テスト: 保存済み応答を用いた refresh と Device Flow の状態遷移、GitHub ページングと PR 除外、Jev 応答検証。実サービスの自己模倣を避け、送信内容と観測可能な結果を確認する。
- 手動確認: 実 GitHub App の Device Flow、実アカウントの repository 権限、実 TypeSafe API、実ターミナルでの表示。

## 追加設計: ブランチ作成

- 既存の Jev リクエストへ Issue ごとの分類 `Choice` question を加える。重要度と分類は独立して判定し、検証済みの結果を同じ Issue に対応付ける。
- 検証済みの Jev 分類と GitHub Issue 番号からブランチ名を組み立てる。Issue タイトルの生成処理や追加の外部 API は設けない。
- ローカルディレクトリの解決と Git 操作を表示処理から分離する。保存済み root と CLI 引数の `repo` から `root/repo` を解決し、クリックされた Issue に限り、指定先と `main` を確認してブランチを作成する。
- 端末表示は Issue 本文下のボタンの描画とマウス操作を対応付ける。操作中は対話状態を維持し、終了時は端末状態を復元する。キーボードからの操作経路も設ける。
- root 設定は公開情報だけを設定ファイルに保存する。認証情報は従来どおり OS の資格情報ストアに置く。
- Git コマンドを使う場合は引数を分離して実行し、対象ディレクトリを明示する。標準出力のテーブル描画に Git の副作用を混ぜない。
