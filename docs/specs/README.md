# docs/specs — Source of Truth

Thư mục này chứa các **spec chuẩn** (source of truth) cho toàn bộ dự án `cli-router`.
Khi hành vi của code và spec ở đây mâu thuẫn, spec là chuẩn — sửa code hoặc cập nhật spec
trong cùng một PR để hai bên luôn đồng bộ.

> Khác với `docs/superpowers/specs/` (spec/plan lịch sử theo từng feature, ghi lại quyết định
> tại thời điểm implement), thư mục này mô tả **trạng thái hiện tại** và được duy trì liên tục.

## Danh sách spec

| File | Nội dung |
|------|----------|
| [`provider-config.md`](provider-config.md) | Cấu hình provider của proxy: ý nghĩa từng field, các `ProviderKind`, kiểu `AuthConfig`, và hướng dẫn cài đặt từng provider (Anthropic, Z.ai, DeepSeek, OpenAI, Codex, MiniMax, Kimi). |

## Quy ước

- Mỗi spec bắt đầu bằng một dòng ghi rõ nó là **source of truth** cho phạm vi nào, kèm
  đường dẫn code tham chiếu.
- Viết bằng tiếng Việt, thuật ngữ kỹ thuật giữ nguyên tiếng Anh.
- Khi thêm spec mới: tạo file trong thư mục này và thêm một dòng vào bảng trên.
- Khi đổi shape dữ liệu / contract: cập nhật spec liên quan **trong cùng commit** với code.
</content>
