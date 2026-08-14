# Provider Config — Spec & Cài đặt

> **Source of truth** cho cấu hình provider của proxy `cli-router`.
> Mọi thay đổi về field cấu hình provider phải đồng bộ với tài liệu này.
>
> Code tham chiếu: `crates/proxy/src/config.rs` (kiểu dữ liệu),
> `crates/proxy-admin-api/src/lib.rs` (payload wire của Admin API),
> `crates/proxy/src/adapters/providers/builder.rs` (wiring),
> `crates/proxy/src/adapters/providers/*.rs` (default URL + xử lý auth từng provider).

Config được lưu trong **SQLite** (`~/.local/share/cli-router/proxy.db`) — single source of
truth duy nhất, serialize dạng **JSON** qua serde. **Không có file config nào để sửa tay.**
Toàn bộ cấu hình được đọc/ghi qua **Admin API** (`GET`/`PUT /admin/config`) hoặc **TUI**
(`cargo run -p proxy-tui`), và có hiệu lực ngay (hot reload).

Các ví dụ dưới đây là **payload JSON** đúng shape mà `PUT /admin/config` nhận (kiểu
`ConfigPayload`). Cách áp dụng nhanh:

```bash
curl -X PUT http://127.0.0.1:8787/admin/config \
  -H "content-type: application/json" \
  -d @config.json
```

---

## 1. Cấu trúc tổng thể (`ConfigPayload`)

```jsonc
{
  "port": 8787,                 // cổng proxy, default 8787
  "providers": [ /* ProviderPayload */ ],   // danh sách provider (chủ đề của spec này)
  "routing":   [ /* RoutingRulePayload */ ], // luật định tuyến model → provider
  "quota":     [ /* QuotaPayload */ ],        // giới hạn quota theo cửa sổ thời gian
  "affinity":  { "enabled": true, "headers": ["x-session-id", "anthropic-session-id"] },
  "proxy_db":  null,            // optional override đường dẫn DB
  "pricing_db": null            // optional override đường dẫn pricing DB
}
```

Ràng buộc validate (`Config::validate`):

- Phải có **ít nhất 1 provider**.
- `name` của provider phải **duy nhất**.
- Phải có **ít nhất 1 routing rule**.
- Mọi `provider` / `fallback` trong routing phải trỏ tới một `name` provider tồn tại.

---

## 2. `ProviderPayload` — ý nghĩa từng field

| Field | Kiểu | Bắt buộc | Default | Áp dụng cho kind | Ý nghĩa |
|-------|------|:---:|---------|------------------|---------|
| `name` | `string` | ✅ | — | mọi kind | Định danh duy nhất. Dùng trong routing (`provider`/`fallback`) và namespace routing (`<name>/<model>`). |
| `kind` | `string` | ✅ | — | — | Loại provider, quyết định endpoint mặc định + cách xử lý format/auth. Xem §3. |
| `auth` | `AuthPayload` | ❌ | `{"type":"passthrough"}` | mọi kind | Cách proxy xác thực **tới** upstream. Xem §4. |
| `base_url` | `string?` | ❌ | theo kind | mọi kind | Ghi đè endpoint chính. Với provider dual-format đây là endpoint **Anthropic-compatible**; với provider OpenAI-only đây là endpoint OpenAI-compatible. |
| `openai_base_url` | `string?` | ❌ | theo kind | `zai`, `minimax`, `kimi` | Ghi đè endpoint **OpenAI-compatible** cho provider dual-format. Bị bỏ qua với các kind khác. |
| `format_mode` | `string?` | ❌ | `"both"` | mọi kind trừ `codex` | Giới hạn endpoint mà proxy được phép dùng: `both` (client nói format nào thì đi thẳng format đó), `anthropic` (luôn đi endpoint Anthropic, dịch client OpenAI), `openai` (luôn đi endpoint OpenAI, dịch client Anthropic). Xem §5b. |
| `thinking_level` | `string?` | ❌ | `"unset"` | mọi kind | Mức suy nghĩ của upstream, chọn trong danh sách riêng của từng kind. Xem §5c. |
| `thinking_force` | `bool?` | ❌ | `false` | mọi kind | `true` thì mức này đè lên tham số client gửi; `false` thì chỉ điền khi client bỏ trống. |
| `thinking_mode` | `string?` | ❌ | `"split_only"` | `minimax` | Cách xử lý nội dung thinking/reasoning trong response. Xem §5. |
| `max_concurrent` | `number?` | ❌ | `null` (dùng default của pool) | mọi kind | Số request đồng thời tối đa tới provider này. Tăng để một provider nhanh phục vụ nhiều request; giảm để tránh vượt rate limit. |
| `model_formats` | `string?` | ❌ | `null` (không có luật nào) | mọi kind, chỉ có tác dụng với `opencode_go` | Bảng model → wire format dạng chuỗi nén. Xem §5d. |

> **Lưu ý về `openai_base_url`:** field này chỉ được builder truyền vào cho `zai`, `minimax`,
> `kimi` (các provider hiểu cả 2 format). `deepseek` và `openai` là OpenAI-only — endpoint của
> chúng cấu hình qua `base_url`. Đặt `openai_base_url` cho các kind này sẽ không có tác dụng.

---

## 3. `kind` — endpoint mặc định & format

`kind` chấp nhận (không phân biệt hoa thường) các alias sau:

| `kind` value | Alias được chấp nhận | Format hỗ trợ | `base_url` default | `openai_base_url` default |
|---|---|---|---|---|
| `anthropic` | — | Anthropic | `https://api.anthropic.com` | — |
| `zai` | `z.ai`, `z-ai` | Anthropic + OpenAI | `https://api.z.ai/api/anthropic` | `https://api.z.ai/api/paas/v4` |
| `deepseek` | `deep-seek` | **OpenAI-only** | `https://api.deepseek.com/v1` | — |
| `openai` | `open_ai` | **OpenAI-only** | `https://api.openai.com/v1` | — |
| `codex` | — | OpenAI (Responses API) | `https://chatgpt.com/backend-api/codex` | — |
| `minimax` | — | Anthropic + OpenAI | `https://api.minimaxi.com/anthropic` | `https://api.minimaxi.com/v1` |
| `kimi` | `moonshot` | Anthropic + OpenAI | `https://api.moonshot.ai/anthropic` | `https://api.moonshot.ai/v1` |
| `opencode_go` | `opencode-go` | Anthropic + OpenAI + Responses (theo model, xem §6.8) | `https://opencode.ai/zen/go` | `https://opencode.ai/zen/go/v1` |

**Format nào đi qua endpoint nào:**
- Request Anthropic (`POST /v1/messages`) → đi qua `base_url` của provider.
- Request OpenAI (`POST /v1/chat/completions`) → đi qua `openai_base_url` (dual-format) hoặc `base_url` (OpenAI-only).
- Nếu client gửi format mà provider không hiểu, proxy dịch chéo (Anthropic↔OpenAI) khi có thể; provider OpenAI-only sẽ lỗi nếu gọi `/v1/messages` mà không dịch được.
- **Ngoại lệ `opencode_go`:** wire format được chọn theo **model**, qua bảng luật `model_formats` (§5d), không theo endpoint client gọi. `select_direction` vẫn dịch chéo như bình thường khi format client dùng khác với format model đó yêu cầu.

---

## 4. `auth` (`AuthPayload`) — cách xác thực tới upstream

`auth` là object tagged bằng field `type`. Secrets luôn bị redact trong log.

| `type` | Field kèm theo | Áp dụng | Hành vi |
|--------|-------|---------|---------|
| `passthrough` (default) | — | mọi kind | Chuyển tiếp nguyên `x-api-key` / `Authorization` client gửi. |
| `api_key` | `value` | mọi kind | Chèn `x-api-key: <value>`. Với endpoint OpenAI-format của `zai`/`minimax`/`kimi`/`deepseek`, tự convert sang `Authorization: Bearer <value>`. |
| `bearer` | `value` | mọi kind | Chèn `Authorization: Bearer <value>`. |
| `anthropic_oauth` | `access_token`, `refresh_token`, `expires_at_ms` | `anthropic` | Session OAuth Anthropic. Proxy tự refresh trước khi hết hạn và retry 1 lần khi gặp 401. |
| `openai_oauth` | `access_token`, `refresh_token`, `expires_at_ms` | `openai`, `codex` | Session OAuth OpenAI. Cùng cơ chế refresh như trên. |
| `codex_auto` | — | `codex`, `openai` | Đọc token từ `~/.codex/auth.json` (cache Codex CLI) lúc khởi động và mỗi lần refresh nền. **Không** lưu token trong DB. |

Ví dụ các dạng `auth`:

```jsonc
{ "type": "passthrough" }
{ "type": "api_key", "value": "sk-ant-..." }
{ "type": "bearer",  "value": "sk-ant-oat-..." }
{ "type": "anthropic_oauth", "access_token": "...", "refresh_token": "...", "expires_at_ms": 1746300000000 }
{ "type": "openai_oauth",    "access_token": "...", "refresh_token": "...", "expires_at_ms": 1746300000000 }
{ "type": "codex_auto" }
```

- `value` được lưu và gửi **nguyên văn**. **Không** có nội suy biến môi trường kiểu `${VAR}` — nếu ghi `"value": "${MOONSHOT_API_KEY}"` thì proxy gửi đúng chuỗi literal `${MOONSHOT_API_KEY}` lên upstream và bị **401**. Dán key thật vào `value`.
- `expires_at_ms` là Unix epoch **millisecond**. Background task refresh khi còn trong vòng 5 phút trước hạn.
- OAuth thường được thiết lập qua TUI (mục §7), không nhập tay.

---

## 5. `thinking_mode` (chỉ `minimax`)

MiniMax (M2.x, M3) sinh nội dung thinking/reasoning. Proxy tự chèn `reasoning_split: true` vào
request để MiniMax tách phần thinking, rồi xử lý theo `thinking_mode`:

| Value | Hành vi |
|-------|---------|
| `split_only` (default) | Bỏ các tag thinking nội bộ trong `content`, **giữ** `reasoning_content` và `reasoning_details` để client hiển thị. |
| `strip_all` | Bỏ **toàn bộ** thinking: tag, `reasoning_content`, `reasoning_details`. |

---

## 5b. `format_mode` — ghim wire format lên upstream

Mặc định `both`: request đi thẳng qua endpoint khớp format của client, không dịch.
Đặt `anthropic` hoặc `openai` khi một trong hai endpoint của upstream bị lỗi và
bạn muốn mọi client đi qua endpoint còn lại (proxy tự dịch chiều còn thiếu).

| Value | Hành vi |
|---|---|
| `both` (default) | Client Anthropic → endpoint Anthropic; client OpenAI → endpoint OpenAI. Chỉ dịch khi provider không phục vụ format của client. |
| `anthropic` | Mọi request đi endpoint Anthropic; client OpenAI được dịch `openai→anthropic`. |
| `openai` | Mọi request đi endpoint OpenAI; client Anthropic được dịch `anthropic→openai`. |

Nếu format được ghim không có URL tương ứng, giá trị bị bỏ qua và provider vẫn
quảng bá các endpoint đã cấu hình — cấu hình sai không làm chết provider.

**Ca dùng thực tế — MiniMax:** endpoint OpenAI của MiniMax tách thinking khỏi câu
trả lời theo thẻ `</think>`, nhưng model thường xuyên phát các ký tự đầu của câu
trả lời *trước* thẻ đó. Phần đó rơi vào `reasoning_content` và client không bao
giờ thấy — biểu hiện là câu trả lời bị mất phần đầu. Endpoint Anthropic tách
đúng, nên đặt `format_mode: "anthropic"` cho `minimax` nếu bạn dùng client nói
OpenAI (opencode).

---

## 5c. `thinking_level` — mức suy nghĩ của upstream

Mỗi kind có một khoang wire khác nhau để điều khiển mức suy nghĩ (`output_config.effort`,
`reasoning.effort`, `reasoning_effort`, hoặc `thinking.type` tùy provider). `thinking_level`
là một field chung duy nhất trừu tượng hóa tất cả các khoang đó: proxy tự dịch giá trị đã
chọn sang đúng shape JSON của kind, merge vào body gửi đi (theo RFC 7396 JSON Merge Patch)
sau khi `format_mode`/dịch định dạng đã quyết định request thực sự đi format nào.

`unset` (default) nghĩa là proxy không chèn gì cả — upstream dùng mặc định của chính nó.
Mỗi kind chỉ cho chọn trong danh sách mức mà upstream của nó thực sự hỗ trợ; admin API và
TUI dùng chung một bảng nên hai nơi không bao giờ lệch nhau:

| kind | Các mức được hỗ trợ |
|---|---|
| `anthropic` | `off` · `low` · `medium` · `high` · `xhigh` · `max` |
| `codex` | `off` · `minimal` · `low` · `medium` · `high` · `xhigh` |
| `openai` | `off` · `minimal` · `low` · `medium` · `high` · `xhigh` |
| `zai` | `off` · `high` · `max` |
| `deepseek` | `high` · `max` |
| `kimi` | `low` · `high` · `max` |
| `minimax` | `off` · `adaptive` |

`thinking_force` quyết định mức này ghi đè hay chỉ điền khuyết:

- **`false`** (default) — proxy chỉ chèn mức đã cấu hình vào các key mà request của client
  **chưa** có sẵn. Client vẫn kiểm soát được theo từng request.
- **`true`** — patch merge đè lên bất kể client gửi gì, mức cấu hình luôn thắng.

Lưu ý theo từng kind:

- **DeepSeek** chấp nhận `low`/`medium` ở tầng wire nhưng upstream tự map cả hai về `high`,
  nên hai mức đó không được liệt kê trong danh sách chọn của `deepseek` — chọn thẳng `high`
  hoặc `max`.
- **MiniMax M2.x** bỏ qua `off`: model vẫn giữ reasoning bật dù `thinking.type` được đặt
  `disabled`. Chỉ M3 tôn trọng giá trị này.
- **Kimi K2.x** trả lỗi nếu request mang **cả** `thinking` lẫn `reasoning_effort` cùng lúc —
  tránh cấu hình chồng lấn thủ công từ phía client khi provider đã có `thinking_level`.

---

## 5d. `model_formats` — ghim wire format theo từng model (chỉ `opencode_go`)

Mọi provider khác chọn wire format theo endpoint client gọi + URL nào được cấu
hình (§3). OpenCode Go là ngoại lệ: format phụ thuộc **model**, không phụ
thuộc endpoint — xem §6.8. `model_formats` là bảng luật cho trường hợp đó,
lưu dưới dạng một chuỗi nén:

```
minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses
```

- **Ngữ pháp:** `glob=format[,glob=format]*`. `format` ∈ `anthropic` |
  `openai` | `responses` — **không** có alias `open_ai` (khác với vocabulary
  của field `kind` ở §3; alias đó đã bị bỏ có chủ đích cho field này).
- Glob dùng cùng matcher với routing rule (`*` là wildcard, `.` là ký tự
  literal, không phải "match mọi ký tự").
- Đoạn rỗng (do dấu phẩy thừa ở cuối, ví dụ `glm-*=openai,`) bị bỏ qua.
- **First-match-wins**, theo đúng thứ tự viết trong chuỗi — không phải luật
  cụ thể nhất thắng.
- Chuỗi rỗng hoặc field để trống (`null`) nghĩa là **không có luật nào** —
  hành vi giống hệt trước khi field này tồn tại: format do URL nào được cấu
  hình quyết định, không phụ thuộc model.
- Được validate trong `Config::validate()` khi `PUT /admin/config` — chuỗi
  sai cú pháp (thiếu `=`, glob rỗng, glob không hợp lệ, hoặc format không nằm
  trong 3 giá trị trên) bị Admin API **từ chối lúc lưu**, không phải lúc có
  request đi qua.

**Luật chỉ được thu hẹp trong phạm vi endpoint provider đã cấu hình.** Nếu một
luật trỏ tới format mà provider không có URL tương ứng (ví dụ đặt
`=anthropic` cho một provider không có `base_url` Anthropic), luật đó bị bỏ
qua cho model đấy — proxy rơi về capability suy ra từ URL, y hệt cách
`format_mode` không bao giờ tự bịa ra endpoint (§5b). Cấu hình sai không làm
model đó "biến mất" khỏi mọi client.

**Cảnh báo khi tự sửa luật thủ công:** một luật `=responses` viết tay trên
provider `minimax`, `deepseek` hay `openai` sẽ âm thầm bỏ qua các quirk theo
kind đó (`reasoning_split`, `strip_tool_choice`) và mọi phép chèn
`thinking_level` — đường Responses không áp dụng quirk hay chèn thinking nào
cả. Bản preset không bao giờ tạo ra tình huống này (chỉ OpenCode Go seed luật
`responses`), nhưng nếu bạn tự thêm luật đó cho provider khác thì các quirk
sẽ mất mà không có cảnh báo nào ở tầng request.

---

## 6. Hướng dẫn cài đặt từng provider

Mỗi provider tối thiểu cần một entry trong `providers` + ít nhất một `routing` rule trỏ tới nó.
Các block dưới đây là phần tử của mảng `providers` / `routing` trong payload `PUT /admin/config`.

### 6.1 Anthropic

```jsonc
// providers[]
{
  "name": "anthropic",
  "kind": "anthropic",
  "auth": { "type": "api_key", "value": "sk-ant-..." }
  // "thinking_level": "high"   // tùy chọn: mức mặc định khi request không chỉ định — xem §5c
}
// routing[]
{ "match": { "model": "claude-*" }, "provider": "anthropic" }
```

- Nhận cả `/v1/messages` và `/v1/chat/completions` (dịch chéo sang Anthropic).
- Auth: `api_key`, `bearer` (token `claude setup-token`), `anthropic_oauth` (auto-refresh), hoặc `passthrough`.

### 6.2 Z.ai (GLM)

```jsonc
// providers[]
{
  "name": "zai",
  "kind": "zai",
  "auth": { "type": "api_key", "value": "<zai-api-key>" },
  "openai_base_url": "https://api.z.ai/api/paas/v4"   // PAYG (default)
}
// routing[]
{ "match": { "model": "glm-*" }, "provider": "zai" }
```

**Coding Plan vs Pay-As-You-Go** — Z.ai định tuyến theo header auth:

| Header gửi lên | Ghi vào ledger |
|---|---|
| `Authorization: Bearer <token>` | Coding Plan |
| `x-api-key: <token>` | Pay-as-you-go |

Nếu dùng **GLM Coding Plan**, cần cả 2:
1. `auth.type = "bearer"` (không phải `api_key`).
2. `openai_base_url = "https://api.z.ai/api/coding/paas/v4"` — tiền tố `coding/` chọn ledger Coding Plan cho request OpenAI-format.

```jsonc
{
  "name": "zai",
  "kind": "zai",
  "auth": { "type": "bearer", "value": "<zai-coding-plan-token>" },
  "openai_base_url": "https://api.z.ai/api/coding/paas/v4"
}
```

Endpoint Anthropic mặc định (`.../api/anthropic`) đã trỏ Coding Plan khi auth Bearer — không cần override cho client dùng Anthropic-format.

### 6.3 DeepSeek

```jsonc
// providers[]
{ "name": "deepseek", "kind": "deepseek", "auth": { "type": "bearer", "value": "<deepseek-key>" } }
// routing[]
{ "match": { "model": "deepseek-*" }, "provider": "deepseek" }
```

- **OpenAI-only**: chỉ nhận `/v1/chat/completions`. Gọi `/v1/messages` sẽ lỗi.
- Endpoint qua `base_url` (default `https://api.deepseek.com/v1`).
- Nên dùng `bearer`. Dùng `api_key` chỉ khi nguồn key mặc định gửi `x-api-key` (proxy tự convert sang Bearer).

### 6.4 OpenAI

```jsonc
// providers[]
{ "name": "openai", "kind": "openai", "auth": { "type": "bearer", "value": "sk-..." } }
// routing[]
{ "match": { "model": "gpt-*" }, "provider": "openai" }
```

- **OpenAI-only**, endpoint default `https://api.openai.com/v1`.
- Auth: `bearer`/`api_key`, hoặc `openai_oauth`/`codex_auto` (OAuth ChatGPT).

### 6.5 Codex

```jsonc
// providers[]
{
  "name": "codex",
  "kind": "codex",
  "auth": { "type": "codex_auto" },   // đọc ~/.codex/auth.json
  "thinking_level": "high"             // tùy chọn — xem §5c
}
// routing[]
{ "match": { "model": "gpt-5-codex*" }, "provider": "codex" }
```

- Endpoint Codex Responses API (`https://chatgpt.com/backend-api/codex`); proxy dịch OpenAI-format sang Responses.
- Auth ưu tiên `codex_auto` (không lưu token trong DB) hoặc `openai_oauth`.
- `thinking_level` được chèn vào body OpenAI-format trước khi dịch, rồi map sang `reasoning.effort` của Responses API.

### 6.6 MiniMax

```jsonc
// providers[]
{
  "name": "minimax",
  "kind": "minimax",
  "auth": { "type": "api_key", "value": "<minimax-key>" },
  "thinking_mode": "split_only"        // hoặc "strip_all"
}
// routing[]
{ "match": { "model": "MiniMax-*" }, "provider": "minimax" }
```

- Dual-format: `/v1/messages` qua `https://api.minimaxi.com/anthropic`, `/v1/chat/completions` qua `https://api.minimaxi.com/v1`.
- Xem §5 về `thinking_mode`.

### 6.7 Kimi (Moonshot)

```jsonc
// providers[]
{
  "name": "kimi",
  "kind": "kimi",                       // alias: "moonshot"
  "auth": { "type": "api_key", "value": "sk-..." }
}
// routing[]
{ "match": { "model": "kimi-*" }, "provider": "kimi" }
```

- Dual-format: `/v1/messages` qua `https://api.moonshot.ai/anthropic`, `/v1/chat/completions` qua `https://api.moonshot.ai/v1`.
- `api_key` được tự convert sang `Bearer` cho endpoint OpenAI-format.

### 6.8 OpenCode Go

```jsonc
// providers[]
{
  "name": "opencode-go",
  "kind": "opencode_go",                // alias: "opencode-go"
  "auth": { "type": "api_key", "value": "<opencode-go-key>" }
  // "model_formats": "minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses"
  //   ↑ giá trị được seed tự động khi tạo provider kind này — xem bên dưới.
}
// routing[]
{ "match": { "model": "*" }, "provider": "opencode-go" }
```

[OpenCode Go](https://opencode.ai/docs/go/) là gói subscription $10/tháng phục
vụ 19 model coding, nhưng **wire format phụ thuộc model, không phụ thuộc
endpoint client gọi** — khác với mọi provider khác trong bảng ở §3. Một
provider `opencode_go` duy nhất phục vụ cả 19 model qua namespace
`opencode-go/<model>`.

- `base_url` (Anthropic-compatible): `https://opencode.ai/zen/go`, client path
  `/v1/messages` được nối vào.
- `openai_base_url` (OpenAI-compatible): `https://opencode.ai/zen/go/v1`,
  path dịch `/chat/completions` được nối vào. Endpoint `/responses` (OpenAI
  Responses API) dùng chung base này, nối `/responses`.
- Auth: preset là `api_key` — `forward` gửi `x-api-key` trên path Anthropic,
  `forward_openai` tự convert sang `Authorization: Bearer` trên path OpenAI,
  giống mọi provider khác dùng `api_key`. **Đây là giả định chưa được xác
  minh** với API thật (chưa có key để probe); nếu `/v1/messages` từ chối
  `x-api-key`, đổi `auth.type` sang `"bearer"`.

**Ba nhóm endpoint** (theo `https://opencode.ai/docs/go/`), điều khiển bởi
`model_formats` seed sẵn ở trên:

| Endpoint | Model |
|---|---|
| `/chat/completions` (fallthrough, không cần luật) | `glm-5.1`, `glm-5.2`, `glm-5.3`, `kimi-k3`, `kimi-k2.7-code`, `kimi-k2.6`, `deepseek-v4-pro`, `deepseek-v4-flash`, `mimo-v2.5`, `mimo-v2.5-pro`, `hy3` |
| `/messages` (luật `=anthropic`) | `minimax-m3`, `minimax-m2.7`, `minimax-m2.5`, `qwen3.8-max`, `qwen3.7-max`, `qwen3.7-plus`, `qwen3.6-plus` |
| `/responses` (luật `=responses`) | `grok-4.5`, `gpt-5.6-luna` |

Một model không khớp luật nào rơi vào `/chat/completions` — đây cũng là nhóm
lớn nhất và ổn định nhất, nên fallthrough an toàn kể cả khi OpenCode thêm
model mới trước khi luật được cập nhật.

- Account usage và quota **không** được hỗ trợ cho kind này
  (`NoopAccountUsage`): OpenCode Go giới hạn theo đô-la ($12/5h, $30/tuần,
  $60/tháng), còn `QuotaRule` của proxy đếm request và token — không có phép
  quy đổi.
- Không có `thinking_level` nào được offer cho kind này; `reasoning_effort`
  client gửi được truyền thẳng qua path Responses.
- Xem §5d để biết ngữ pháp và ràng buộc đầy đủ của `model_formats`.

> **Trạng thái xác minh:** các giá trị URL, auth và bảng model ở trên lấy từ
> tài liệu OpenCode Go và từ preset đã implement; **chưa được gọi thử với API
> thật** (cần subscription key). Xem "Assumptions and risks" trong
> `docs/superpowers/specs/2026-08-14-opencode-go-provider-design.md`.

---

## 7. Thiết lập OAuth (Anthropic / OpenAI)

Không nhập token tay — dùng TUI:

```bash
cargo run -p proxy       # terminal 1
cargo run -p proxy-tui   # terminal 2
```

Trong TUI: chọn provider → **Edit Auth** → chọn OAuth → trình duyệt mở trang authorize →
đăng nhập → copy `code` (phần trước `#`) dán lại vào TUI. Config được ghi `*_oauth` và
background task tự refresh (mỗi 60s, khi còn <5 phút trước hạn; retry 1 lần nếu gặp 401).

---

## 8. Nhiều tài khoản cùng kind

Đặt nhiều provider cùng `kind` khác `name`, rồi dùng routing/load-balancing hoặc namespace
(`<name>/<model>`) để chọn:

```jsonc
// providers[]
{ "name": "anthropic-work",     "kind": "anthropic", "auth": { "type": "api_key", "value": "sk-ant-work-..." } },
{ "name": "anthropic-personal", "kind": "anthropic", "auth": { "type": "api_key", "value": "sk-ant-personal-..." } }
// routing[]
{
  "match": { "model": "*" },
  "strategy": "round_robin",
  "provider": "anthropic-work",
  "fallback": ["anthropic-personal"]
}
```

---

## Tham chiếu chéo

- Routing / load balancing / priority / namespace: xem `RoutingRulePayload` và `RoutingStrategy` trong code.
- Kiểu dữ liệu config: `crates/proxy/src/config.rs`; payload wire: `crates/proxy-admin-api/src/lib.rs`.
- Default URL & xử lý auth từng provider: `crates/proxy/src/adapters/providers/{anthropic,zai,deepseek,openai,codex,minimax,kimi}.rs`.
- `opencode_go` (default URL, auth, seed `model_formats`): `crates/proxy/src/adapters/providers/upstream.rs` (`default_urls`, `preset_model_formats`).
- `model_formats` (parse + validate): `crates/proxy/src/config.rs` (`parse_model_formats`); bảng luật đã compile: `crates/proxy/src/adapters/providers/model_formats.rs`.
</content>
