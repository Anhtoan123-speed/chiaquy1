# StellarSplit (Soroban Smart Contract)

Mô tả ngắn
-----------
StellarSplit là một hợp đồng Soroban (Rust) để quản lý quỹ chia tiền nhóm (USDC) trên Stellar Testnet. Hợp đồng cho phép:

- Khởi tạo nhóm với một trưởng nhóm (leader).
- Thêm / xóa thành viên.
- Nạp USDC vào quỹ (leader hoặc thành viên đã approve hợp đồng).
- Trưởng nhóm chia đều số dư quỹ cho tất cả thành viên (split).
- Trao quyền rút tiền (Full / Limited) và rút tiền khỏi quỹ.
- Truy vấn thông tin nhóm, danh sách thành viên, và trạng thái quyền rút.

Tệp chính
---------
- `lib.rs` — toàn bộ mã hợp đồng và bộ test nội bộ (unit tests).

Hàm/Interface chính (public contract methods)
--------------------------------------------
- `init(leader, group_name, usdc_token)` — khởi tạo hợp đồng (gọi 1 lần).
- `join(caller, member_addr, nickname)` — leader thêm thành viên.
- `remove_member(caller, member_addr)` — leader xóa thành viên (không thể xóa leader).
- `deposit(from, amount)` — chuyển USDC từ `from` vào contract (phải approve trước).
- `split(caller)` — leader chia đều số dư hiện có cho tất cả thành viên.
- `grant_withdraw(caller, grantee, role, limit)` — leader cấp quyền rút.
- `revoke_withdraw(caller, grantee)` — leader thu hồi quyền rút.
- `withdraw(caller, amount)` — thành viên đã được cấp quyền rút tiền.
- `get_withdraw_role(env, addr)` — xem quyền rút của một địa chỉ.
- `get_info()` — trả về `ContractInfo` (tên nhóm, leader, thống kê quỹ, v.v.).
- `get_members()` — trả về danh sách thành viên kèm số tiền đã nhận.
- `get_balance()` — số dư USDC hiện có trong quỹ.

Storage (DataKey)
------------------
- `GroupInfo` — thông tin chung: tên nhóm, leader, token USDC, created_at.
- `Members` — `Vec<Member>`; mỗi `Member` có `addr`, `nickname`, `total_received`.
- `TotalDeposited`, `TotalSplit`, `TotalWithdrawn` — thống kê số tiền (i128).
- `SplitCount` — số lần chia đã thực hiện (u32).
- `WithdrawPerm(Address)` — `WithdrawPermission` cho từng địa chỉ (role, limit, total_withdrawn).

Lỗi contract (SplitError)
-------------------------
- `NotInitialized = 1` — chưa gọi `init`.
- `AlreadyInitialized = 2` — đã init rồi.
- `NotLeader = 3` — caller không phải leader.
- `AlreadyMember = 4`, `NoMembers = 5`, `ZeroAmount = 6`, `InsufficientFunds = 7`, `GroupFull = 8`,
- `CannotRemoveLeader = 9`, `MemberNotFound = 10`, `NoWithdrawRole = 11`, `ExceedsLimit = 12`, `NotMember = 13`.

Ví dụ ngắn (tư duy khi test trong `#[cfg(test)]`)
------------------------------------------------
- Trong test, repo đang tạo mock Stellar Asset và dùng `Env::default()` + `env.mock_all_auths()`
- Hàm `setup()` deploy contract, tạo token mock và mint USDC cho leader.
- Đơn vị token sử dụng ở code là "stroops": 1 USDC = 10_000_000. Có helper `fn usdc(amount: i128) -> i128`.

Chạy build & test (PowerShell trên Windows)
-----------------------------------------
Mở PowerShell trong thư mục chứa `Cargo.toml` (nếu đây là crate), rồi:

```powershell
# Build (debug)
cargo build

# Chạy unit tests (tests trong lib.rs)
cargo test
```

Ghi chú: test trong file `lib.rs` dùng SDK Soroban test utilities (mock env). `cargo test` sẽ chạy các test đó.

Gợi ý phát triển và kiểm tra nhanh
----------------------------------
- Mỗi khi thay đổi logic lưu trữ, chạy `cargo test` để đảm bảo không phá vỡ invariant.
- Nếu cần debug log trong test, dùng `log!(&env, ...)` đã có sẵn trong code; output test sẽ hiển thị log.

Tiếp theo / Nâng cấp đề xuất
---------------------------
- Thêm file `README.md` này vào repo (đã thực hiện).
- (Tùy chọn) Thêm ví dụ CLI nhỏ hoặc script để deploy contract lên Soroban localnet / testnet.
- (Tùy chọn) Tách module và thêm các unit tests cho edge-case (ví dụ: chia khi share == 0, timeout/ttl behaviors).

Checklist
---------
- [x] Tạo `README.md` mô tả mục đích, API, lưu trữ, lỗi, và hướng dẫn build/test (PowerShell).

License
-------
Không chỉ định rõ trong dự án; thêm license nếu bạn muốn public (MIT/Apache recommended).

---

Nếu bạn muốn, mình có thể thêm một script PowerShell ví dụ để deploy và gọi một số hàm demo (init → join → deposit → split).
