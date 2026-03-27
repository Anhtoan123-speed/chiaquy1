//! StellarSplit — Soroban Smart Contract
//!
//! Quản lý quỹ chia tiền nhóm sinh viên trên Stellar Testnet.
//!
//! Luồng cơ bản:
//!   1. Trưởng nhóm gọi `init` để khởi tạo nhóm và đặt tên.
//!   2. Các thành viên gọi `join` để đăng ký vào nhóm.
//!   3. Trưởng nhóm gọi `deposit` chuyển USDC vào quỹ.
//!   4. Trưởng nhóm gọi `split` để chia đều và gửi về ví mỗi thành viên.
//!   5. Bất kỳ ai gọi `get_info` / `get_members` để xem trạng thái.
//!
//! Hệ thống trao quyền rút tiền (`WithdrawRole`):
//!   - `None`      : Không có quyền rút (mặc định mọi thành viên)
//!   - `Limited`   : Rút được tối đa `withdraw_limit` USDC mỗi lần
//!   - `Full`      : Rút không giới hạn (chỉ leader mới trao được)
//!
//!   Hàm liên quan:
//!   - `grant_withdraw`  : Leader trao quyền và đặt hạn mức cho một địa chỉ
//!   - `revoke_withdraw` : Leader thu hồi quyền rút
//!   - `withdraw`        : Thành viên được uỷ quyền rút USDC ra khỏi quỹ
//!   - `get_withdraw_role`: Xem quyền hiện tại của một địa chỉ

#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror,
    token::Client as TokenClient,
    Address, Env, String, Vec,
    log, panic_with_error,
};

// ── Lỗi contract ────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum SplitError {
    /// Contract chưa được khởi tạo
    NotInitialized    = 1,
    /// Contract đã được khởi tạo rồi
    AlreadyInitialized = 2,
    /// Người gọi không phải trưởng nhóm
    NotLeader         = 3,
    /// Địa chỉ đã là thành viên
    AlreadyMember     = 4,
    /// Nhóm chưa có thành viên nào
    NoMembers         = 5,
    /// Số tiền phải lớn hơn 0
    ZeroAmount        = 6,
    /// Số dư quỹ không đủ để chia
    InsufficientFunds = 7,
    /// Nhóm đã đạt giới hạn thành viên (tối đa 20)
    GroupFull         = 8,
    /// Trưởng nhóm không thể tự xóa bản thân
    CannotRemoveLeader = 9,
    /// Thành viên không tồn tại trong nhóm
    MemberNotFound    = 10,
    /// Địa chỉ không có quyền rút tiền
    NoWithdrawRole    = 11,
    /// Số tiền rút vượt hạn mức được phép (Limited role)
    ExceedsLimit      = 12,
    /// Địa chỉ không phải thành viên, không thể trao quyền
    NotMember         = 13,
}

// ── Kiểu dữ liệu lưu trữ ────────────────────────────────────────────────────

/// Khóa lưu trong storage
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Thông tin chung của nhóm
    GroupInfo,
    /// Danh sách thành viên
    Members,
    /// Tổng USDC đã nạp vào quỹ
    TotalDeposited,
    /// Tổng USDC đã chia ra
    TotalSplit,
    /// Số lần chia đã thực hiện
    SplitCount,
    /// Quyền rút của một địa chỉ cụ thể: DataKey::WithdrawPerm(addr)
    WithdrawPerm(Address),
    /// Tổng USDC đã rút khỏi quỹ (không qua split)
    TotalWithdrawn,
}

/// Cấp độ quyền rút tiền từ quỹ
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum WithdrawRole {
    /// Không có quyền rút (mặc định)
    None,
    /// Rút được tối đa `limit` USDC mỗi lần gọi
    Limited,
    /// Rút không giới hạn số lượng
    Full,
}

/// Thông tin quyền rút của một địa chỉ
#[contracttype]
#[derive(Clone, Debug)]
pub struct WithdrawPermission {
    /// Cấp độ quyền
    pub role: WithdrawRole,
    /// Hạn mức tối đa mỗi lần rút (chỉ áp dụng khi role = Limited)
    /// Đơn vị: stroops (1 USDC = 10_000_000)
    pub limit: i128,
    /// Tổng số USDC đã rút kể từ khi được cấp quyền
    pub total_withdrawn: i128,
}

/// Thông tin nhóm
#[contracttype]
#[derive(Clone, Debug)]
pub struct GroupInfo {
    /// Tên nhóm (VD: "Phòng 3B Ký túc xá")
    pub name: String,
    /// Địa chỉ trưởng nhóm
    pub leader: Address,
    /// Địa chỉ token USDC trên Testnet
    pub usdc_token: Address,
    /// Timestamp khởi tạo
    pub created_at: u64,
}

/// Thông tin một thành viên
#[contracttype]
#[derive(Clone, Debug)]
pub struct Member {
    /// Địa chỉ Stellar của thành viên
    pub addr: Address,
    /// Bí danh (VD: "Minh", "An")
    pub nickname: String,
    /// Tổng USDC đã nhận từ các lần chia
    pub total_received: i128,
}

/// Kết quả trả về khi gọi `get_info`
#[contracttype]
#[derive(Clone, Debug)]
pub struct ContractInfo {
    pub group: GroupInfo,
    pub member_count: u32,
    pub total_deposited: i128,
    pub total_split: i128,
    pub total_withdrawn: i128,
    pub split_count: u32,
    pub current_balance: i128,
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct StellarSplitContract;

#[contractimpl]
impl StellarSplitContract {

    // ── Khởi tạo ──────────────────────────────────────────────────────────

    /// Khởi tạo nhóm. Chỉ gọi một lần duy nhất.
    ///
    /// # Tham số
    /// - `leader`     : Địa chỉ trưởng nhóm (người gọi hàm này)
    /// - `group_name` : Tên hiển thị của nhóm
    /// - `usdc_token` : Địa chỉ contract USDC trên Testnet
    pub fn init(
        env: Env,
        leader: Address,
        group_name: String,
        usdc_token: Address,
    ) {
        // Chặn gọi lại nếu đã init
        if env.storage().instance().has(&DataKey::GroupInfo) {
            panic_with_error!(&env, SplitError::AlreadyInitialized);
        }

        leader.require_auth();

        let info = GroupInfo {
            name: group_name.clone(),
            leader: leader.clone(),
            usdc_token,
            created_at: env.ledger().timestamp(),
        };

        // Leader tự động là thành viên đầu tiên
        let first_member = Member {
            addr: leader.clone(),
            nickname: String::from_str(&env, "Leader"),
            total_received: 0,
        };

        let mut members: Vec<Member> = Vec::new(&env);
        members.push_back(first_member);

        env.storage().instance().set(&DataKey::GroupInfo,       &info);
        env.storage().instance().set(&DataKey::Members,         &members);
        env.storage().instance().set(&DataKey::TotalDeposited,  &0_i128);
        env.storage().instance().set(&DataKey::TotalSplit,      &0_i128);
        env.storage().instance().set(&DataKey::TotalWithdrawn,  &0_i128);
        env.storage().instance().set(&DataKey::SplitCount,      &0_u32);

        // Kéo dài TTL của storage (khoảng 30 ngày ledger)
        env.storage().instance().extend_ttl(2_592_000, 2_592_000);

        log!(&env, "StellarSplit: nhóm '{}' đã khởi tạo bởi {}", group_name, leader);
    }

    // ── Quản lý thành viên ────────────────────────────────────────────────

    /// Thêm thành viên vào nhóm. Chỉ trưởng nhóm mới có quyền gọi.
    ///
    /// # Tham số
    /// - `member_addr` : Địa chỉ Stellar của thành viên mới
    /// - `nickname`    : Bí danh để hiển thị
    pub fn join(
        env: Env,
        caller: Address,
        member_addr: Address,
        nickname: String,
    ) {
        caller.require_auth();
        let info = Self::require_init(&env);

        // Chỉ leader có quyền thêm thành viên
        if caller != info.leader {
            panic_with_error!(&env, SplitError::NotLeader);
        }

        let mut members: Vec<Member> = env
            .storage().instance()
            .get(&DataKey::Members)
            .unwrap();

        // Giới hạn 20 thành viên
        if members.len() >= 20 {
            panic_with_error!(&env, SplitError::GroupFull);
        }

        // Kiểm tra trùng lặp
        for m in members.iter() {
            if m.addr == member_addr {
                panic_with_error!(&env, SplitError::AlreadyMember);
            }
        }

        members.push_back(Member {
            addr: member_addr.clone(),
            nickname: nickname.clone(),
            total_received: 0,
        });

        env.storage().instance().set(&DataKey::Members, &members);
        env.storage().instance().extend_ttl(2_592_000, 2_592_000);

        log!(&env, "StellarSplit: thêm thành viên {} ({})", nickname, member_addr);
    }

    /// Xóa thành viên khỏi nhóm. Chỉ trưởng nhóm mới có quyền gọi.
    pub fn remove_member(env: Env, caller: Address, member_addr: Address) {
        caller.require_auth();
        let info = Self::require_init(&env);

        if caller != info.leader {
            panic_with_error!(&env, SplitError::NotLeader);
        }

        // Không cho phép xóa chính leader
        if member_addr == info.leader {
            panic_with_error!(&env, SplitError::CannotRemoveLeader);
        }

        let mut members: Vec<Member> = env
            .storage().instance()
            .get(&DataKey::Members)
            .unwrap();

        let before = members.len();
        let mut new_members: Vec<Member> = Vec::new(&env);

        for m in members.iter() {
            if m.addr != member_addr {
                new_members.push_back(m);
            }
        }

        if new_members.len() == before {
            panic_with_error!(&env, SplitError::MemberNotFound);
        }

        env.storage().instance().set(&DataKey::Members, &new_members);
        log!(&env, "StellarSplit: đã xóa thành viên {}", member_addr);
    }

    // ── Quỹ & chia tiền ───────────────────────────────────────────────────

    /// Nạp USDC vào quỹ nhóm.
    ///
    /// Người gọi phải đã approve contract này được phép
    /// chi tiêu ít nhất `amount` USDC từ ví của họ.
    ///
    /// # Tham số
    /// - `from`   : Địa chỉ người nạp tiền
    /// - `amount` : Số USDC nạp (đơn vị: stroops = 1/10_000_000 USDC)
    pub fn deposit(env: Env, from: Address, amount: i128) {
        from.require_auth();
        let info = Self::require_init(&env);

        if amount <= 0 {
            panic_with_error!(&env, SplitError::ZeroAmount);
        }

        // Chuyển USDC từ ví người dùng vào contract
        let token = TokenClient::new(&env, &info.usdc_token);
        token.transfer(&from, &env.current_contract_address(), &amount);

        // Cập nhật tổng đã nạp
        let mut total: i128 = env
            .storage().instance()
            .get(&DataKey::TotalDeposited)
            .unwrap_or(0);
        total += amount;
        env.storage().instance().set(&DataKey::TotalDeposited, &total);
        env.storage().instance().extend_ttl(2_592_000, 2_592_000);

        log!(&env, "StellarSplit: {} nạp {} USDC vào quỹ", from, amount);
    }

    /// Chia đều toàn bộ số dư quỹ cho tất cả thành viên.
    ///
    /// Chỉ trưởng nhóm mới có quyền kích hoạt lệnh chia.
    /// Phần dư (do chia không hết) sẽ được giữ lại trong quỹ cho lần sau.
    pub fn split(env: Env, caller: Address) {
        caller.require_auth();
        let info = Self::require_init(&env);

        if caller != info.leader {
            panic_with_error!(&env, SplitError::NotLeader);
        }

        let mut members: Vec<Member> = env
            .storage().instance()
            .get(&DataKey::Members)
            .unwrap();

        let n = members.len();
        if n == 0 {
            panic_with_error!(&env, SplitError::NoMembers);
        }

        // Lấy số dư hiện tại của contract
        let token = TokenClient::new(&env, &info.usdc_token);
        let balance = token.balance(&env.current_contract_address());

        if balance <= 0 {
            panic_with_error!(&env, SplitError::InsufficientFunds);
        }

        // Chia đều, phần dư giữ lại
        let share = balance / (n as i128);
        if share == 0 {
            panic_with_error!(&env, SplitError::InsufficientFunds);
        }

        let total_sent = share * (n as i128);

        // Chuyển tiền cho từng thành viên và cập nhật lịch sử
        let mut updated: Vec<Member> = Vec::new(&env);
        for mut m in members.iter() {
            token.transfer(&env.current_contract_address(), &m.addr, &share);
            m.total_received += share;
            updated.push_back(m.clone());
            log!(&env, "StellarSplit: gửi {} USDC → {}", share, m.addr);
        }

        // Cập nhật storage
        let mut total_split: i128 = env
            .storage().instance()
            .get(&DataKey::TotalSplit)
            .unwrap_or(0);
        total_split += total_sent;

        let mut split_count: u32 = env
            .storage().instance()
            .get(&DataKey::SplitCount)
            .unwrap_or(0);
        split_count += 1;

        env.storage().instance().set(&DataKey::Members,    &updated);
        env.storage().instance().set(&DataKey::TotalSplit, &total_split);
        env.storage().instance().set(&DataKey::SplitCount, &split_count);
        env.storage().instance().extend_ttl(2_592_000, 2_592_000);

        log!(
            &env,
            "StellarSplit: lần chia #{} — {} người × {} USDC = {} tổng",
            split_count, n, share, total_sent
        );
    }

    // ── Trao quyền rút tiền ───────────────────────────────────────────────

    /// Trao quyền rút tiền cho một thành viên. Chỉ trưởng nhóm mới có quyền gọi.
    ///
    /// # Tham số
    /// - `caller`  : Phải là leader
    /// - `grantee` : Địa chỉ được trao quyền (phải là thành viên trong nhóm)
    /// - `role`    : `WithdrawRole::Limited` hoặc `WithdrawRole::Full`
    /// - `limit`   : Hạn mức mỗi lần rút tính bằng stroops
    ///               (chỉ có ý nghĩa khi role = Limited; truyền 0 nếu role = Full)
    pub fn grant_withdraw(
        env: Env,
        caller: Address,
        grantee: Address,
        role: WithdrawRole,
        limit: i128,
    ) {
        caller.require_auth();
        let info = Self::require_init(&env);

        if caller != info.leader {
            panic_with_error!(&env, SplitError::NotLeader);
        }

        // Chỉ trao quyền cho thành viên đã có trong nhóm
        let members: Vec<Member> = env
            .storage().instance()
            .get(&DataKey::Members)
            .unwrap();

        let is_member = members.iter().any(|m| m.addr == grantee);
        if !is_member {
            panic_with_error!(&env, SplitError::NotMember);
        }

        // Không cho phép trao None (dùng revoke_withdraw)
        if role == WithdrawRole::None {
            panic_with_error!(&env, SplitError::NoWithdrawRole);
        }

        // Nếu là Limited thì limit phải > 0
        if role == WithdrawRole::Limited && limit <= 0 {
            panic_with_error!(&env, SplitError::ZeroAmount);
        }

        let perm = WithdrawPermission {
            role: role.clone(),
            limit,
            total_withdrawn: 0,
        };

        env.storage().instance().set(&DataKey::WithdrawPerm(grantee.clone()), &perm);
        env.storage().instance().extend_ttl(2_592_000, 2_592_000);

        log!(&env, "StellarSplit: trao quyền rút cho {}", grantee);
    }

    /// Thu hồi quyền rút tiền của một địa chỉ. Chỉ trưởng nhóm mới có quyền gọi.
    pub fn revoke_withdraw(env: Env, caller: Address, grantee: Address) {
        caller.require_auth();
        let info = Self::require_init(&env);

        if caller != info.leader {
            panic_with_error!(&env, SplitError::NotLeader);
        }

        env.storage().instance().remove(&DataKey::WithdrawPerm(grantee.clone()));
        env.storage().instance().extend_ttl(2_592_000, 2_592_000);

        log!(&env, "StellarSplit: thu hồi quyền rút của {}", grantee);
    }

    /// Rút USDC ra khỏi quỹ nhóm về ví của người gọi.
    ///
    /// Người gọi phải có quyền `Limited` hoặc `Full`.
    /// - `Full`    : rút bất kỳ số lượng nào, miễn quỹ đủ.
    /// - `Limited` : số tiền mỗi lần không được vượt `limit` đã được thiết lập.
    ///
    /// # Tham số
    /// - `caller` : Địa chỉ người rút (phải có WithdrawPermission)
    /// - `amount` : Số USDC muốn rút (stroops)
    pub fn withdraw(env: Env, caller: Address, amount: i128) {
        caller.require_auth();
        let info = Self::require_init(&env);

        if amount <= 0 {
            panic_with_error!(&env, SplitError::ZeroAmount);
        }

        // Lấy quyền của người gọi
        let mut perm: WithdrawPermission = env
            .storage().instance()
            .get(&DataKey::WithdrawPerm(caller.clone()))
            .unwrap_or(WithdrawPermission {
                role: WithdrawRole::None,
                limit: 0,
                total_withdrawn: 0,
            });

        match perm.role {
            WithdrawRole::None => {
                panic_with_error!(&env, SplitError::NoWithdrawRole);
            }
            WithdrawRole::Limited => {
                // Kiểm tra hạn mức mỗi lần rút
                if amount > perm.limit {
                    panic_with_error!(&env, SplitError::ExceedsLimit);
                }
            }
            WithdrawRole::Full => {
                // Không giới hạn số lượng
            }
        }

        // Kiểm tra quỹ đủ
        let token = TokenClient::new(&env, &info.usdc_token);
        let balance = token.balance(&env.current_contract_address());
        if amount > balance {
            panic_with_error!(&env, SplitError::InsufficientFunds);
        }

        // Thực hiện chuyển tiền
        token.transfer(&env.current_contract_address(), &caller, &amount);

        // Cập nhật lịch sử rút của người này
        perm.total_withdrawn += amount;
        env.storage().instance().set(&DataKey::WithdrawPerm(caller.clone()), &perm);

        // Cập nhật tổng đã rút toàn quỹ
        let mut total_withdrawn: i128 = env
            .storage().instance()
            .get(&DataKey::TotalWithdrawn)
            .unwrap_or(0);
        total_withdrawn += amount;
        env.storage().instance().set(&DataKey::TotalWithdrawn, &total_withdrawn);
        env.storage().instance().extend_ttl(2_592_000, 2_592_000);

        log!(&env, "StellarSplit: {} rút {} USDC khỏi quỹ", caller, amount);
    }

    /// Xem quyền rút tiền hiện tại của một địa chỉ.
    /// Trả về `None` nếu địa chỉ không có quyền rút.
    pub fn get_withdraw_role(env: Env, addr: Address) -> WithdrawPermission {
        Self::require_init(&env);
        env.storage().instance()
            .get(&DataKey::WithdrawPerm(addr))
            .unwrap_or(WithdrawPermission {
                role: WithdrawRole::None,
                limit: 0,
                total_withdrawn: 0,
            })
    }

    // ── Query (đọc dữ liệu) ───────────────────────────────────────────────

    /// Trả về toàn bộ thông tin nhóm và thống kê quỹ.
    pub fn get_info(env: Env) -> ContractInfo {
        let info    = Self::require_init(&env);
        let members: Vec<Member> = env
            .storage().instance()
            .get(&DataKey::Members)
            .unwrap();

        let total_deposited: i128 = env
            .storage().instance()
            .get(&DataKey::TotalDeposited)
            .unwrap_or(0);

        let total_split: i128 = env
            .storage().instance()
            .get(&DataKey::TotalSplit)
            .unwrap_or(0);

        let total_withdrawn: i128 = env
            .storage().instance()
            .get(&DataKey::TotalWithdrawn)
            .unwrap_or(0);

        let split_count: u32 = env
            .storage().instance()
            .get(&DataKey::SplitCount)
            .unwrap_or(0);

        let token = TokenClient::new(&env, &info.usdc_token);
        let current_balance = token.balance(&env.current_contract_address());

        ContractInfo {
            group: info,
            member_count: members.len(),
            total_deposited,
            total_split,
            total_withdrawn,
            split_count,
            current_balance,
        }
    }

    /// Trả về danh sách thành viên kèm số tiền đã nhận.
    pub fn get_members(env: Env) -> Vec<Member> {
        Self::require_init(&env);
        env.storage().instance()
            .get(&DataKey::Members)
            .unwrap()
    }

    /// Trả về số dư USDC hiện tại trong quỹ.
    pub fn get_balance(env: Env) -> i128 {
        let info = Self::require_init(&env);
        let token = TokenClient::new(&env, &info.usdc_token);
        token.balance(&env.current_contract_address())
    }

    // ── Helpers nội bộ ────────────────────────────────────────────────────

    fn require_init(env: &Env) -> GroupInfo {
        env.storage()
            .instance()
            .get(&DataKey::GroupInfo)
            .unwrap_or_else(|| panic_with_error!(env, SplitError::NotInitialized))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation},
        token::{Client as TokenClient, StellarAssetClient},
        Address, Env, IntoVal, String,
    };

    // ── Helpers test ─────────────────────────────────────────────────────

    fn setup() -> (Env, Address, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();

        // Deploy contract
        let contract_id = env.register_contract(None, StellarSplitContract);

        // Tạo mock USDC token (Stellar Asset Contract)
        let usdc_admin = Address::generate(&env);
        let usdc_id    = env.register_stellar_asset_contract_v2(usdc_admin.clone());
        let usdc_addr  = usdc_id.address();

        let leader = Address::generate(&env);

        // Mint USDC cho leader để test
        StellarAssetClient::new(&env, &usdc_addr)
            .mint(&leader, &100_000_0000000); // 100,000 USDC (7 decimals)

        (env, contract_id, usdc_addr, leader, usdc_admin)
    }

    fn usdc(amount: i128) -> i128 { amount * 10_000_000 } // 7 decimals

    // ── Test 1: Khởi tạo nhóm ────────────────────────────────────────────

    #[test]
    fn test_init_ok() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);

        client.init(
            &leader,
            &String::from_str(&env, "Phòng 3B Ký túc xá"),
            &usdc,
        );

        let info = client.get_info();
        assert_eq!(info.member_count, 1);
        assert_eq!(info.total_deposited, 0);
        assert_eq!(info.split_count, 0);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn test_init_twice_fails() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);

        client.init(&leader, &String::from_str(&env, "Nhóm A"), &usdc);
        // Gọi lần 2 phải panic với AlreadyInitialized (code 2)
        client.init(&leader, &String::from_str(&env, "Nhóm B"), &usdc);
    }

    // ── Test 2: Quản lý thành viên ───────────────────────────────────────

    #[test]
    fn test_join_member_ok() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        let bob   = Address::generate(&env);

        client.join(&leader, &alice, &String::from_str(&env, "Alice"));
        client.join(&leader, &bob,   &String::from_str(&env, "Bob"));

        let info = client.get_info();
        assert_eq!(info.member_count, 3); // leader + alice + bob
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #4)")]
    fn test_join_duplicate_fails() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));
        client.join(&leader, &alice, &String::from_str(&env, "Alice2")); // AlreadyMember (#4)
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #3)")]
    fn test_non_leader_cannot_add_member() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let intruder = Address::generate(&env);
        let victim   = Address::generate(&env);
        // Kẻ lạ không có quyền thêm thành viên → NotLeader (#3)
        client.join(&intruder, &victim, &String::from_str(&env, "Victim"));
    }

    #[test]
    fn test_remove_member_ok() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));
        assert_eq!(client.get_info().member_count, 2);

        client.remove_member(&leader, &alice);
        assert_eq!(client.get_info().member_count, 1);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #9)")]
    fn test_cannot_remove_leader() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        // Leader không thể xóa chính mình (#9)
        client.remove_member(&leader, &leader);
    }

    // ── Test 3: Nạp tiền ─────────────────────────────────────────────────

    #[test]
    fn test_deposit_ok() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        // Approve và nạp 300 USDC
        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(300), &200);
        client.deposit(&leader, &usdc(300));

        let info = client.get_info();
        assert_eq!(info.total_deposited, usdc(300));
        assert_eq!(info.current_balance,  usdc(300));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #6)")]
    fn test_deposit_zero_fails() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        client.deposit(&leader, &0); // ZeroAmount (#6)
    }

    // ── Test 4: Chia tiền ────────────────────────────────────────────────

    #[test]
    fn test_split_equal_shares() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        let bob   = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));
        client.join(&leader, &bob,   &String::from_str(&env, "Bob"));

        // Nạp 300 USDC → mỗi người nhận 100 USDC
        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(300), &200);
        client.deposit(&leader, &usdc(300));

        let leader_before = token.balance(&leader);
        let alice_before  = token.balance(&alice);
        let bob_before    = token.balance(&bob);

        client.split(&leader);

        // Mỗi người phải nhận đúng 100 USDC
        assert_eq!(token.balance(&leader) - leader_before, usdc(100));
        assert_eq!(token.balance(&alice)  - alice_before,  usdc(100));
        assert_eq!(token.balance(&bob)    - bob_before,    usdc(100));

        // Quỹ phải trống
        assert_eq!(client.get_balance(), 0);

        let info = client.get_info();
        assert_eq!(info.split_count, 1);
        assert_eq!(info.total_split, usdc(300));
    }

    #[test]
    fn test_split_remainder_stays_in_fund() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));
        // 2 thành viên: leader + alice

        // Nạp 101 USDC → mỗi người nhận 50, dư 1 USDC trong quỹ
        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(101), &200);
        client.deposit(&leader, &usdc(101));
        client.split(&leader);

        // Phần dư 1 USDC phải còn trong contract
        assert_eq!(client.get_balance(), usdc(1));
    }

    #[test]
    fn test_multiple_splits() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));

        let token = TokenClient::new(&env, &usdc);

        // Lần chia 1: nạp 200 → chia 100/người
        token.approve(&leader, &cid, &usdc(200), &200);
        client.deposit(&leader, &usdc(200));
        client.split(&leader);

        // Lần chia 2: nạp thêm 400 → chia 200/người
        token.approve(&leader, &cid, &usdc(400), &400);
        client.deposit(&leader, &usdc(400));
        client.split(&leader);

        let info = client.get_info();
        assert_eq!(info.split_count, 2);
        assert_eq!(info.total_split, usdc(600));

        // Kiểm tra total_received của từng thành viên
        let members = client.get_members();
        for m in members.iter() {
            assert_eq!(m.total_received, usdc(300)); // 100 + 200
        }
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #3)")]
    fn test_non_leader_cannot_split() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let intruder = Address::generate(&env);
        client.split(&intruder); // NotLeader (#3)
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #7)")]
    fn test_split_empty_fund_fails() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        client.split(&leader); // InsufficientFunds (#7) — quỹ trống
    }

    // ── Test 6: Trao quyền & rút tiền ───────────────────────────────────

    #[test]
    fn test_grant_full_role_and_withdraw() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));

        // Nạp 500 USDC vào quỹ
        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(500), &200);
        client.deposit(&leader, &usdc(500));

        // Trao quyền Full cho Alice
        client.grant_withdraw(&leader, &alice, &WithdrawRole::Full, &0);

        let perm = client.get_withdraw_role(&alice);
        assert_eq!(perm.role, WithdrawRole::Full);

        // Alice rút 200 USDC
        let alice_before = token.balance(&alice);
        client.withdraw(&alice, &usdc(200));

        assert_eq!(token.balance(&alice) - alice_before, usdc(200));
        assert_eq!(client.get_balance(), usdc(300));

        // total_withdrawn phải được ghi nhận
        let info = client.get_info();
        assert_eq!(info.total_withdrawn, usdc(200));

        // Lịch sử rút của Alice
        let perm_after = client.get_withdraw_role(&alice);
        assert_eq!(perm_after.total_withdrawn, usdc(200));
    }

    #[test]
    fn test_grant_limited_role_within_limit() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let bob = Address::generate(&env);
        client.join(&leader, &bob, &String::from_str(&env, "Bob"));

        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(1000), &200);
        client.deposit(&leader, &usdc(1000));

        // Bob chỉ được rút tối đa 100 USDC mỗi lần
        client.grant_withdraw(&leader, &bob, &WithdrawRole::Limited, &usdc(100));

        let bob_before = token.balance(&bob);
        client.withdraw(&bob, &usdc(100)); // Đúng hạn mức → OK
        assert_eq!(token.balance(&bob) - bob_before, usdc(100));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #12)")]
    fn test_limited_role_exceeds_limit_fails() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let bob = Address::generate(&env);
        client.join(&leader, &bob, &String::from_str(&env, "Bob"));

        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(1000), &200);
        client.deposit(&leader, &usdc(1000));

        // Hạn mức 50 USDC nhưng Bob cố rút 200 → ExceedsLimit (#12)
        client.grant_withdraw(&leader, &bob, &WithdrawRole::Limited, &usdc(50));
        client.withdraw(&bob, &usdc(200));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #11)")]
    fn test_no_role_cannot_withdraw() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));

        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(500), &200);
        client.deposit(&leader, &usdc(500));

        // Alice chưa được trao quyền → NoWithdrawRole (#11)
        client.withdraw(&alice, &usdc(100));
    }

    #[test]
    fn test_revoke_withdraw_blocks_future_withdraw() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));

        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(500), &200);
        client.deposit(&leader, &usdc(500));

        // Cấp rồi thu hồi
        client.grant_withdraw(&leader, &alice, &WithdrawRole::Full, &0);
        client.revoke_withdraw(&leader, &alice);

        let perm = client.get_withdraw_role(&alice);
        assert_eq!(perm.role, WithdrawRole::None);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #11)")]
    fn test_revoked_user_cannot_withdraw() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));

        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(500), &200);
        client.deposit(&leader, &usdc(500));

        client.grant_withdraw(&leader, &alice, &WithdrawRole::Full, &0);
        client.revoke_withdraw(&leader, &alice);

        // Alice đã bị thu hồi → NoWithdrawRole (#11)
        client.withdraw(&alice, &usdc(100));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #13)")]
    fn test_cannot_grant_non_member() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let outsider = Address::generate(&env);
        // Outsider chưa join → NotMember (#13)
        client.grant_withdraw(&leader, &outsider, &WithdrawRole::Full, &0);
    }

    #[test]
    fn test_withdraw_reduces_balance_correctly() {
        let (env, cid, usdc, leader, _) = setup();
        let client = StellarSplitContractClient::new(&env, &cid);
        client.init(&leader, &String::from_str(&env, "Nhóm test"), &usdc);

        let alice = Address::generate(&env);
        let bob   = Address::generate(&env);
        client.join(&leader, &alice, &String::from_str(&env, "Alice"));
        client.join(&leader, &bob,   &String::from_str(&env, "Bob"));

        let token = TokenClient::new(&env, &usdc);
        token.approve(&leader, &cid, &usdc(900), &200);
        client.deposit(&leader, &usdc(900));

        // Trao quyền cho cả Alice (Full) và Bob (Limited 150)
        client.grant_withdraw(&leader, &alice, &WithdrawRole::Full,    &0);
        client.grant_withdraw(&leader, &bob,   &WithdrawRole::Limited, &usdc(150));

        client.withdraw(&alice, &usdc(300));
        client.withdraw(&bob,   &usdc(150));

        // Quỹ còn: 900 - 300 - 150 = 450
        assert_eq!(client.get_balance(), usdc(450));

        let info = client.get_info();
        assert_eq!(info.total_withdrawn, usdc(450));

        // Vẫn có thể split phần còn lại cho 3 thành viên: 450/3 = 150 mỗi người
        client.split(&leader);
        assert_eq!(client.get_balance(), 0);
    }
}
