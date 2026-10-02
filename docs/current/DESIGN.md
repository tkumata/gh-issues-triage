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
      └─ 全 Issue を1回の Jev リクエストで重要度・ブランチ分類・Ready 判定
          └─ 全回答を検証して重要度順に安定ソート
              └─ 端末幅に合わせてテーブルを描画
```

## 責務

| 責務 | 内容 |
| --- | --- |
| CLI | `<owner>/<repo>` の検証、usage、終了コード |
| GitHub authentication | 保存済み token の利用、refresh、必要時の Device Flow、GitHub エラーの解釈 |
| credential storage | OS の資格情報ストアを使った GitHub App user access token と refresh token の保存と読み込み |
| GitHub issues | REST API ページング、pull request 除外、最大10件への制限 |
| triage | Jev request の構築、Score・Choice 応答検証、安定ソート |
| presentation | Unicode 表示幅に基づく折り返し、列幅配分、罫線描画 |

実装時のファイル分割は責務が読みにくくならない最小単位とし、責務ごとのファイル作成を必須にしない。

## 主要データ

```text
RepositoryRef { owner, repo }
Issue { number, title, body, source_order }
RankedIssue { issue, score, prefix, readiness }
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
- structured state と Issue ごとの Score・Choice question を1リクエストにまとめる。
- question の5段階 criterion はアプリ側の仕様として固定し、返却された Score を独自変換しない。
- 返却された answer の件数、type、有限数、許容範囲を検証してから使用する。

### 端末

- 端末幅の取得と Unicode 表示幅の計算は標準ライブラリで不足する境界であり、実装時に小さな既存 crate を選定してよい。
- テーブル描画そのものは要求が限定されるため、汎用 UI 層を作らない。
- `presentation` は Issue ごとの物理行範囲を `IssueRegion` に記録する。折り返し・幅計算・入力の制御文字除去を済ませてから、セル単位で ANSI の黒文字・bright blue 背景を付加する。各縦罫線の前で前景・背景を既定色へ戻し、罫線は装飾しない。
- 一覧の選択 index と表示のスクロール位置を別々に保持する。`navigate_issue` は `j`/`k`・上下矢印で Issue 単位に選択を移動し、選択対象が見える位置へスクロールする。マウス移動は表示座標とスクロール位置から選択 index を更新する。
- `scroll_by` は PageUp/PageDown による画面単位のスクロールと、結果画面の `j`/`k`・上下矢印による1行スクロールを扱う。マウス移動の通知を有効にし、終了時は既存の端末復元処理で無効にする。

## エラー方針

- 利用者が直せる内容を先頭に示し、HTTP 応答や内部エラーの連鎖を保持する。
- timeout、レート制限、認証拒否、期限切れ、権限不足、repository 不在、TypeSafe 応答不正、端末幅不足を区別する。
- 認証前、保存完了前、全 Score 検証前には成功を報告しない。

## 検証境界

- 単体テスト: 引数解析、repository 解析、Score 検証、安定ソート、Unicode 折り返し、列幅と罫線。
- 一覧操作の回帰テスト: 全件が一画面に収まる場合の選択遷移、上下端、長い本文、マウス座標とスクロール補正、マウスからキーへの切り替え、Page スクロール、セルの色付けと枠線の維持。
- HTTP 境界テスト: 保存済み応答を用いた refresh と Device Flow の状態遷移、GitHub ページングと PR 除外、Jev 応答検証。実サービスの自己模倣を避け、送信内容と観測可能な結果を確認する。
- 手動確認: 実 GitHub App の Device Flow、実アカウントの repository 権限、実 TypeSafe API、実ターミナルでの表示。

### 一覧ハイライト改修の確認結果（2026-10-01）

- `make check` 成功（41テスト）、`make build` 成功。Rust 差分のレビューで修正が必要な指摘なし。
- 一覧のハイライトと操作は、ユーザーによる実端末での動作確認完了。今回の確認記録は表示・操作の改修に限定し、認証・外部 API の受け入れ結果とは区別する。

## 追加設計: ブランチ作成

- 既存の Jev リクエストへ Issue ごとの分類 `Choice` question を加える。重要度と分類は独立して判定し、検証済みの結果を同じ Issue に対応付ける。
- 検証済みの Jev 分類と GitHub Issue 番号からブランチ名を組み立てる。Issue タイトルの生成処理や追加の外部 API は設けない。
- ローカルディレクトリの解決と Git 操作を表示処理から分離する。保存済み root と CLI 引数の `repo` から `root/repo` を解決し、クリックされた Issue に限り、指定先と `main` を確認してブランチを作成する。
- 端末表示は Issue 本文下のボタンの描画とマウス操作を対応付ける。操作中は対話状態を維持し、終了時は端末状態を復元する。キーボードからの操作経路も設ける。
- root 設定は公開情報だけを設定ファイルに保存する。認証情報は従来どおり OS の資格情報ストアに置く。
- Git コマンドを使う場合は引数を分離して実行し、対象ディレクトリを明示する。標準出力のテーブル描画に Git の副作用を混ぜない。

## 追加設計: ブランチ状態の表示

- 表示位置・記号・凡例は2026-10-01にユーザー承認済み。2026-10-01に実装済み。実端末での表示確認は未実施。
- `branch` は、既存の root 解決・Git root・origin 検証を利用し、ローカルブランチの読み取りを担当する。作成時の `main` 検証と副作用は存在確認に含めない。
- 読み取ったブランチ名と Issue 番号の照合は Git コマンド実行と分離する。既存の許可 prefix と完全一致する `issue-<number>` を照合し、Jev の今回の分類に依存させない。
- `main` は一覧表示前に一度確認し、Issue ごとの存在・未存在・確認不能と、確認不能の理由を表示処理へ渡す。状態は永続保存せず、一覧表示中のポーリングも行わない。
- この画面での作成成功後は同じ確認・描画処理を再実行する。`RenderedTable` は凡例と確認不能の理由を折り返した `branch_help` を持ち、`main` が操作ヘルプへ追加して表示領域の高さを計算する。
- `presentation` は渡された状態を番号右隣の `🌿`・空欄・`?`、凡例、理由へ変換する。Git や設定ファイルにはアクセスしない。既存の Unicode 幅計算、折り返し、セルのハイライト処理を利用する。
- ブランチ状態の確認失敗だけを一覧継続の対象とし、理由を保持する。その他のエラー方針と作成操作の契約は維持する。
- 新しいモジュール、汎用 Git 層、設定、依存関係は設けない。

### 検証境界

- ローカル Git の確認: ブランチあり・なし、異なる許可 prefix、Issue 番号の完全一致、remote-tracking ref の除外、root 未設定、origin 不一致、Git の確認失敗。存在確認で現在のブランチと作業ファイルが変わらないことを確認する。
- 表示: 3状態の位置と幅、凡例と理由、絵文字を含む行の端末幅、仮選択ハイライト、既存のクリック・キー操作への対応を確認する。
- 実装後は既定の `make check` と `make build` を使用する。実端末での絵文字の幅・罫線・操作の確認は自動チェックと区別して記録する。

### ブランチ状態表示の確認結果（2026-10-01）

- `make check` 成功（43テスト）、`make build` 成功。ローカルブランチの照合、確認の副作用がないこと、確認不能の理由、3状態の位置と幅、折り返し、既存のクリック領域・仮選択の回帰を自動検証した。
- 実端末での絵文字の表示幅・罫線・操作、および作成成功後に一覧へ戻ったときの状態更新は手動確認未実施。

## 追加設計: 着手可能度（Ready）

2026-10-02にユーザー承認済み。実装済みで、実端末の表示・操作確認は未実施。既存の責務分割とエラー方針を維持する。

- `jev` は既存のリクエストへ Issue ごとの Ready `Choice` question を追加し、重要度・ブランチ分類・Ready の回答を同じ Issue に対応付ける。3つの判定は独立させ、外部 API の往復を追加しない。
- 検証済みの判定を3値の `Readiness` として `RankedIssue` に保持する。API の選択肢と表示文字列は `Yes`、`Needs information`、`Needs investigation` に対応させる。未回答・不正回答を表す既定値は設けない。
- `presentation` は渡された Ready を表示し、判定や API アクセスを行わない。既存の Unicode 幅計算、罫線、セル装飾、クリック領域と Issue 行範囲の計算を4列へ対応させる。
- `main` から表示までの受け渡しは既存の `RankedIssue` を使用する。Ready の値を並べ替えやブランチ作成条件には使用しない。
- 変更対象は既存の Jev 判定・結果型・表示と、その動作を確認するテストに限定する。新しいモジュール、汎用分類層、設定、依存関係、永続化は追加しない。
- 実装対象は `src/jev.rs`、`src/model.rs`、`src/presentation.rs`。型変更に伴い `src/main.rs` と `src/branch.rs` のテストデータも更新する。変更後の結果型は `RankedIssue { issue, score, prefix, readiness }` とする。

### 検証境界

- リクエスト: 同じ state と1回のリクエストに3種の question が含まれ、Ready が対象 Issue のパスと4観点を参照することを確認する。
- 応答: 3値の取得と Issue の対応付け、回答の欠落・型不正・未知の選択肢・不正な確率の拒否を確認する。Ready が異なる場合も重要度順と同順位の順序を維持する。
- 表示と操作: 3つの文字列、0件、複数行、Unicode を含む Issue、端末幅不足、ハイライト、既存の番号右隣のブランチ状態、クリック・キー操作への対応を確認する。
- 実装後は既定の `make check` と `make build` を使用する。文書更新だけの段階では実装検証の成功を記録しない。
- 実 Jev の判定品質は自動応答テストと区別し、情報不足・事前調査・着手可能の代表例、不具合以外の再現条件不要の例、両方不足する例で手動確認する。モデル出力の確率的な性質を踏まえ、判定例を完全一致する単体テストとして扱わない。
- 実端末で4列の幅・罫線・仮選択・ブランチ作成操作を確認し、自動検証と区別して記録する。

### Ready 追加の確認結果（2026-10-02）

- `make check` 成功（45テスト）、`make build` 成功。3値の対応付け、欠落・不正回答の拒否、確率検証、Ready に依存しない重要度順、4列の幅・表示・ハイライト・クリック領域、既存の選択操作とブランチ作成を自動検証した。
- 実 Jev へ実装の Ready instructions と criteria を使って5例を1回送信し、情報不足、事前調査、具体的な文書修正、再現手順不要の機能追加、情報不足と調査必要の併存について、期待する3値と一致した。代表例の確認であり、判定品質全体を保証するものではない。
- 実端末で4列の幅・罫線・仮選択・ブランチ作成操作を確認する手動検証は未実施。
