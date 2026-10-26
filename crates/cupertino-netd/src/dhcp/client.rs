//! The DHCPv4 client state machine of RFC 2131 (figure 5), driven by time and messages.
//!
//! The machine does no I/O and reads no clock: the host passes the current monotonic time to
//! every call and carries out the returned [`Action`]s. INIT-REBOOT is not implemented: a client
//! that starts always discovers.
//!
//! ```text
//! INIT --DISCOVER--> SELECTING --OFFER, REQUEST--> REQUESTING --ACK--> BOUND
//!                                                                        | T1
//!  lease expires / NAK  <---  REBINDING  <---T2---  RENEWING  <----------+
//! ```

use std::net::Ipv4Addr;
use std::time::Duration;

use cupertino_netlink::MacAddr;

use super::message::{Message, MessageType};

/// A monotonic time: the time since some fixed origin the host chooses.
pub type Timestamp = Duration;

/// Retransmission delays of DISCOVER and REQUEST: "exponential backoff ... starting at 4 seconds
/// and doubling up to 64 seconds" (RFC 2131, 4.1). The ±1 s randomisation the RFC adds is omitted
/// (approximate): the machine is deterministic, so a host that wants jitter adds it to the timers.
const RETRANSMIT_DELAYS: [Duration; 5] = [Duration::from_secs(4), Duration::from_secs(8), Duration::from_secs(16), Duration::from_secs(32), Duration::from_secs(64)];

/// How long a client waits after a NAK before it discovers again: "a minimum of ten seconds"
/// (RFC 2131, 3.1 item 5, to avoid looping).
const NAK_HOLDOFF: Duration = Duration::from_secs(10);

/// The shortest wait between renewal and rebinding retransmissions (RFC 2131, 4.4.5).
const MIN_RENEW_RETRY: Duration = Duration::from_secs(60);

/// T1 is half the lease and T2 is 0.875 of it unless the server says otherwise (RFC 2131, 4.5).
const DEFAULT_T1_NUMERATOR: u32 = 1;
const DEFAULT_T1_DENOMINATOR: u32 = 2;
const DEFAULT_T2_NUMERATOR: u32 = 7;
const DEFAULT_T2_DENOMINATOR: u32 = 8;

/// The options a client asks for: subnet mask, router, DNS servers, domain name, lease time, T1
/// and T2 (RFC 2132 numbers).
const PARAMETER_REQUEST_LIST: [u8; 7] = [1, 3, 6, 15, 51, 58, 59];

/// What the client needs from the host to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientConfig {
    pub hardware: MacAddr,
    /// The first transaction id; each transaction after it adds one. The host seeds it randomly.
    pub xid_seed: u32,
}

/// The states of RFC 2131.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Init,
    Selecting,
    Requesting,
    Bound,
    Renewing,
    Rebinding,
}

/// What the server leased.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub address: Ipv4Addr,
    pub prefix_len: u8,
    pub routers: Vec<Ipv4Addr>,
    pub dns_servers: Vec<Ipv4Addr>,
    pub domain_name: Option<String>,
    pub server: Ipv4Addr,
    /// `None` is an infinite lease.
    pub duration: Option<Duration>,
}

/// Something the host must do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send `message` in an IPv4 packet to `destination` (a server, or the broadcast address).
    Send { message: Message, destination: Ipv4Addr },
    /// The server acknowledged: configure the address, routes and DNS. Sent for every ACK; compare
    /// with the previous lease to see whether anything but the times changed.
    Bound(Lease),
    /// The lease ended (it expired, or the server refused the renewal): remove the configuration.
    Lost(Lease),
    /// No server answered for the whole retransmission schedule (124 s). The client goes on
    /// discovering.
    GaveUp,
}

/// A lease and the times that follow from it.
#[derive(Debug, Clone)]
struct Binding {
    lease: Lease,
    t1: Option<Timestamp>,
    t2: Option<Timestamp>,
    expires: Option<Timestamp>,
}

/// The client of one interface.
#[derive(Debug)]
pub struct Client {
    config: ClientConfig,
    state: State,
    transaction: u32,
    xid: u32,
    /// When the current acquisition or renewal began, for the `secs` field.
    began: Timestamp,
    /// When the first copy of the current REQUEST was sent: the lease counts from there.
    request_sent: Timestamp,
    /// The OFFER being requested: server and address.
    offer: Option<(Ipv4Addr, Ipv4Addr)>,
    /// The index into [`RETRANSMIT_DELAYS`] of the delay running now.
    attempt: usize,
    /// When to send again, or when INIT may discover again.
    timer: Option<Timestamp>,
    binding: Option<Binding>,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            state: State::Init,
            transaction: 0,
            xid: config.xid_seed,
            began: Duration::ZERO,
            request_sent: Duration::ZERO,
            offer: None,
            attempt: 0,
            timer: None,
            binding: None,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The lease in force: `Some` in BOUND, RENEWING and REBINDING.
    pub fn lease(&self) -> Option<&Lease> {
        self.binding.as_ref().map(|binding| &binding.lease)
    }

    /// Begins acquiring a lease: INIT to SELECTING. Call once, and again after [`Client::release`].
    pub fn start(&mut self, now: Timestamp) -> Vec<Action> {
        self.discover(now)
    }

    /// When [`Client::tick`] next has something to do.
    pub fn next_deadline(&self) -> Option<Timestamp> {
        match self.state {
            State::Init | State::Selecting | State::Requesting => self.timer,
            State::Bound => self.binding.as_ref().and_then(|binding| [binding.t1, binding.expires].into_iter().flatten().min()),
            State::Renewing => self.binding.as_ref().and_then(|binding| [self.timer, binding.t2, binding.expires].into_iter().flatten().min()),
            State::Rebinding => self.binding.as_ref().and_then(|binding| [self.timer, binding.expires].into_iter().flatten().min()),
        }
    }

    /// Runs every timer that is due at `now`.
    pub fn tick(&mut self, now: Timestamp) -> Vec<Action> {
        let mut actions = Vec::new();
        // The loop ends: each pass moves the state forward or clears the due timer.
        loop {
            let before = (self.state, self.timer, self.attempt);
            match self.state {
                State::Init => {
                    if self.timer.is_some_and(|due| due <= now) {
                        actions.extend(self.discover(now));
                    }
                }
                State::Selecting | State::Requesting => {
                    if self.timer.is_some_and(|due| due <= now) {
                        actions.extend(self.retransmit_or_give_up(now));
                    }
                }
                State::Bound | State::Renewing | State::Rebinding => actions.extend(self.tick_bound(now)),
            }
            if (self.state, self.timer, self.attempt) == before {
                return actions;
            }
        }
    }

    /// Handles a message received from a server.
    pub fn receive(&mut self, now: Timestamp, message: &Message) -> Vec<Action> {
        if message.xid != self.xid || message.client_hardware != self.config.hardware {
            return Vec::new();
        }
        match (self.state, message.message_type) {
            (State::Selecting, MessageType::Offer) => self.on_offer(now, message),
            (State::Requesting | State::Renewing | State::Rebinding, MessageType::Ack) => self.on_ack(message),
            (State::Requesting, MessageType::Nak) => {
                self.enter_init(now + NAK_HOLDOFF);
                Vec::new()
            }
            (State::Renewing | State::Rebinding, MessageType::Nak) => {
                let lost = self.binding.take().map(|binding| Action::Lost(binding.lease));
                self.enter_init(now + NAK_HOLDOFF);
                lost.into_iter().collect()
            }
            _ => Vec::new(),
        }
    }

    /// Gives the lease back (DHCPRELEASE, RFC 2131, 4.4.6) and returns to INIT without a timer.
    pub fn release(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        if let Some(binding) = self.binding.take() {
            let mut message = self.message(MessageType::Release, Duration::ZERO);
            message.client_address = binding.lease.address;
            message.options.server_id = Some(binding.lease.server);
            actions.push(Action::Send { message, destination: binding.lease.server });
            actions.push(Action::Lost(binding.lease));
        }
        self.state = State::Init;
        self.timer = None;
        self.offer = None;
        actions
    }

    fn new_transaction(&mut self) {
        self.xid = self.config.xid_seed.wrapping_add(self.transaction);
        self.transaction = self.transaction.wrapping_add(1);
    }

    /// A message of this client for the current transaction.
    fn message(&self, message_type: MessageType, now: Timestamp) -> Message {
        let mut message = Message::new(message_type, self.xid, self.config.hardware);
        message.seconds = u16::try_from(now.saturating_sub(self.began).as_secs()).unwrap_or(u16::MAX);
        let mut client_id = vec![1];
        client_id.extend_from_slice(&self.config.hardware.0);
        message.options.client_id = Some(client_id);
        message
    }

    fn enter_init(&mut self, timer: Timestamp) {
        self.state = State::Init;
        self.timer = Some(timer);
        self.offer = None;
        self.binding = None;
    }

    /// INIT to SELECTING: a new transaction and a DISCOVER.
    fn discover(&mut self, now: Timestamp) -> Vec<Action> {
        self.new_transaction();
        self.began = now;
        self.state = State::Selecting;
        self.attempt = 0;
        self.timer = Some(now + RETRANSMIT_DELAYS[0]);
        vec![self.send_discover(now)]
    }

    fn send_discover(&self, now: Timestamp) -> Action {
        let mut message = self.message(MessageType::Discover, now);
        message.broadcast = true;
        message.options.parameter_request_list = PARAMETER_REQUEST_LIST.to_vec();
        Action::Send { message, destination: Ipv4Addr::BROADCAST }
    }

    fn send_selecting_request(&self, now: Timestamp) -> Action {
        let (server, address) = self.offer.expect("a request in REQUESTING follows an offer");
        let mut message = self.message(MessageType::Request, now);
        message.broadcast = true;
        message.options.requested_address = Some(address);
        message.options.server_id = Some(server);
        message.options.parameter_request_list = PARAMETER_REQUEST_LIST.to_vec();
        Action::Send { message, destination: Ipv4Addr::BROADCAST }
    }

    /// RENEWING sends to the server, REBINDING to everyone, both from the leased address with
    /// `ciaddr` set and without the requested-address and server options (RFC 2131, 4.3.2).
    fn send_renewal_request(&self, now: Timestamp, destination: Ipv4Addr) -> Action {
        let lease = &self.binding.as_ref().expect("renewal needs a lease").lease;
        let mut message = self.message(MessageType::Request, now);
        message.client_address = lease.address;
        message.options.parameter_request_list = PARAMETER_REQUEST_LIST.to_vec();
        Action::Send { message, destination }
    }

    fn on_offer(&mut self, now: Timestamp, offer: &Message) -> Vec<Action> {
        let Some(server) = offer.options.server_id else {
            return Vec::new();
        };
        if offer.your_address.is_unspecified() {
            return Vec::new();
        }
        self.offer = Some((server, offer.your_address));
        self.state = State::Requesting;
        self.attempt = 0;
        self.request_sent = now;
        self.timer = Some(now + RETRANSMIT_DELAYS[0]);
        vec![self.send_selecting_request(now)]
    }

    /// Reads the lease out of an ACK; `None` when the server left out an option a lease cannot do without
    /// or the subnet mask is not a prefix.
    fn lease_of(ack: &Message) -> Option<(Lease, Option<u32>, Option<u32>)> {
        let options = &ack.options;
        let mask = u32::from(options.subnet_mask?);
        let prefix_len = u8::try_from(mask.leading_ones()).ok().filter(|ones| mask.checked_shl(u32::from(*ones)).unwrap_or(0) == 0)?;
        let seconds = options.lease_time?;
        let lease = Lease {
            address: ack.your_address,
            prefix_len,
            routers: options.routers.clone(),
            dns_servers: options.dns_servers.clone(),
            domain_name: options.domain_name.clone(),
            server: options.server_id?,
            duration: (seconds != u32::MAX).then(|| Duration::from_secs(u64::from(seconds))),
        };
        Some((lease, options.renewal_time, options.rebinding_time))
    }

    fn on_ack(&mut self, ack: &Message) -> Vec<Action> {
        let Some((lease, renewal, rebinding)) = Self::lease_of(ack) else {
            return Vec::new();
        };
        if lease.address.is_unspecified() {
            return Vec::new();
        }
        // the lease counts from the first copy of the REQUEST (RFC 2131, 4.4.5)
        let start = self.request_sent;
        let binding = Binding {
            t1: lease.duration.map(|duration| start + timer_after(duration, renewal, DEFAULT_T1_NUMERATOR, DEFAULT_T1_DENOMINATOR)),
            t2: lease.duration.map(|duration| start + timer_after(duration, rebinding, DEFAULT_T2_NUMERATOR, DEFAULT_T2_DENOMINATOR)),
            expires: lease.duration.map(|duration| start + duration),
            lease: lease.clone(),
        };
        self.binding = Some(binding);
        self.state = State::Bound;
        self.timer = None;
        self.offer = None;
        vec![Action::Bound(lease)]
    }

    fn retransmit_or_give_up(&mut self, now: Timestamp) -> Vec<Action> {
        if self.attempt + 1 < RETRANSMIT_DELAYS.len() {
            self.attempt += 1;
            self.timer = Some(now + RETRANSMIT_DELAYS[self.attempt]);
            let send = if self.state == State::Selecting { self.send_discover(now) } else { self.send_selecting_request(now) };
            return vec![send];
        }
        let mut actions = vec![Action::GaveUp];
        actions.extend(self.discover(now));
        actions
    }

    /// BOUND, RENEWING and REBINDING: the lease times decide the state at `now`.
    fn tick_bound(&mut self, now: Timestamp) -> Vec<Action> {
        let binding = self.binding.clone().expect("a bound state has a binding");
        let due = |at: Option<Timestamp>| at.is_some_and(|at| at <= now);
        if due(binding.expires) {
            self.binding = None;
            self.enter_init(now);
            let mut actions = vec![Action::Lost(binding.lease)];
            actions.extend(self.discover(now));
            return actions;
        }
        if due(binding.t2) && self.state != State::Rebinding {
            self.state = State::Rebinding;
            self.began = now;
            return self.renewal_attempt(now, Ipv4Addr::BROADCAST, &binding, true);
        }
        if due(binding.t1) && self.state == State::Bound {
            self.state = State::Renewing;
            self.began = now;
            return self.renewal_attempt(now, binding.lease.server, &binding, true);
        }
        if matches!(self.state, State::Renewing | State::Rebinding) && due(self.timer) {
            let destination = if self.state == State::Renewing { binding.lease.server } else { Ipv4Addr::BROADCAST };
            return self.renewal_attempt(now, destination, &binding, false);
        }
        Vec::new()
    }

    /// Sends a renewal REQUEST and sets the next retransmission: half the time left until T2
    /// (RENEWING) or until expiry (REBINDING), at least 60 s (RFC 2131, 4.4.5). A new request of
    /// a state starts a new transaction.
    fn renewal_attempt(&mut self, now: Timestamp, destination: Ipv4Addr, binding: &Binding, first: bool) -> Vec<Action> {
        if first {
            self.new_transaction();
            self.request_sent = now;
        }
        let until = if self.state == State::Renewing { binding.t2.or(binding.expires) } else { binding.expires };
        let remaining = until.map_or(MIN_RENEW_RETRY, |until| until.saturating_sub(now));
        self.timer = Some(now + (remaining / 2).max(MIN_RENEW_RETRY));
        vec![self.send_renewal_request(now, destination)]
    }
}

/// The time from the start of the lease to a timer: the server's value, else `numerator /
/// denominator` of the lease.
fn timer_after(lease: Duration, server_value: Option<u32>, numerator: u32, denominator: u32) -> Duration {
    server_value.map_or_else(|| lease * numerator / denominator, |seconds| Duration::from_secs(u64::from(seconds)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dhcp::message::Options;

    const MAC: MacAddr = MacAddr([2, 0, 0, 0, 0, 1]);
    const SERVER: Ipv4Addr = Ipv4Addr::new(192, 168, 7, 1);
    const ADDRESS: Ipv4Addr = Ipv4Addr::new(192, 168, 7, 20);

    fn at(seconds: u64) -> Timestamp {
        Duration::from_secs(seconds)
    }

    fn client() -> Client {
        Client::new(ClientConfig { hardware: MAC, xid_seed: 1000 })
    }

    fn sent(actions: &[Action]) -> Vec<(MessageType, Ipv4Addr)> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Send { message, destination } => Some((message.message_type, *destination)),
                _ => None,
            })
            .collect()
    }

    fn server_message(message_type: MessageType, xid: u32, lease_seconds: u32, renewal: Option<u32>, rebinding: Option<u32>) -> Message {
        let mut message = Message::new(message_type, xid, MAC);
        message.your_address = ADDRESS;
        message.options = Options {
            subnet_mask: Some(Ipv4Addr::new(255, 255, 255, 0)),
            routers: vec![SERVER],
            dns_servers: vec![SERVER],
            server_id: Some(SERVER),
            lease_time: Some(lease_seconds),
            renewal_time: renewal,
            rebinding_time: rebinding,
            ..Options::default()
        };
        message
    }

    /// Takes a client through DISCOVER, OFFER, REQUEST and ACK at time 0 and returns it bound to a
    /// 1000 s lease with the default T1 (500 s) and T2 (875 s).
    fn bound_client() -> Client {
        let mut client = client();
        let actions = client.start(at(0));
        let Action::Send { message, .. } = &actions[0] else { panic!("expected a send") };
        let xid = message.xid;
        client.receive(at(0), &server_message(MessageType::Offer, xid, 1000, None, None));
        client.receive(at(0), &server_message(MessageType::Ack, xid, 1000, None, None));
        assert_eq!(client.state(), State::Bound);
        client
    }

    fn xid_of(actions: &[Action]) -> u32 {
        actions.iter().find_map(|action| if let Action::Send { message, .. } = action { Some(message.xid) } else { None }).expect("a message was sent")
    }

    #[test]
    fn a_full_lease_discover_offer_request_ack() {
        let mut client = client();
        let discover = client.start(at(0));
        assert_eq!(sent(&discover), [(MessageType::Discover, Ipv4Addr::BROADCAST)]);
        let Action::Send { message, .. } = &discover[0] else { unreachable!() };
        assert_eq!(message.xid, 1000);
        assert!(message.broadcast);
        assert_eq!(message.options.parameter_request_list, [1, 3, 6, 15, 51, 58, 59]);
        assert_eq!(message.options.client_id, Some(vec![1, 2, 0, 0, 0, 0, 1]));
        assert_eq!(client.state(), State::Selecting);

        let request = client.receive(at(1), &server_message(MessageType::Offer, 1000, 1000, None, None));
        assert_eq!(sent(&request), [(MessageType::Request, Ipv4Addr::BROADCAST)]);
        let Action::Send { message, .. } = &request[0] else { unreachable!() };
        assert_eq!((message.options.requested_address, message.options.server_id, message.client_address), (Some(ADDRESS), Some(SERVER), Ipv4Addr::UNSPECIFIED));
        assert_eq!(client.state(), State::Requesting);

        let bound = client.receive(at(1), &server_message(MessageType::Ack, 1000, 1000, None, None));
        assert_eq!(
            bound,
            [Action::Bound(Lease {
                address: ADDRESS,
                prefix_len: 24,
                routers: vec![SERVER],
                dns_servers: vec![SERVER],
                domain_name: None,
                server: SERVER,
                duration: Some(at(1000)),
            })]
        );
        assert_eq!(client.state(), State::Bound);
        // the lease counts from the REQUEST (1 s), so T1 is 1 + 500
        assert_eq!(client.next_deadline(), Some(at(501)));
    }

    #[test]
    fn messages_of_another_transaction_or_client_are_ignored() {
        let mut client = client();
        client.start(at(0));
        assert!(client.receive(at(1), &server_message(MessageType::Offer, 999, 1000, None, None)).is_empty());
        let mut other = server_message(MessageType::Offer, 1000, 1000, None, None);
        other.client_hardware = MacAddr([2, 0, 0, 0, 0, 9]);
        assert!(client.receive(at(1), &other).is_empty());
        assert_eq!(client.state(), State::Selecting);
    }

    #[test]
    fn an_offer_without_a_server_identifier_is_ignored() {
        let mut client = client();
        client.start(at(0));
        let mut offer = server_message(MessageType::Offer, 1000, 1000, None, None);
        offer.options.server_id = None;
        assert!(client.receive(at(1), &offer).is_empty());
        assert_eq!(client.state(), State::Selecting);
    }

    #[test]
    fn an_ack_without_a_lease_time_or_with_a_ragged_mask_is_ignored() {
        let mut client = client();
        client.start(at(0));
        client.receive(at(0), &server_message(MessageType::Offer, 1000, 1000, None, None));
        let mut ack = server_message(MessageType::Ack, 1000, 1000, None, None);
        ack.options.lease_time = None;
        assert!(client.receive(at(1), &ack).is_empty());
        let mut ragged = server_message(MessageType::Ack, 1000, 1000, None, None);
        ragged.options.subnet_mask = Some(Ipv4Addr::new(255, 0, 255, 0));
        assert!(client.receive(at(1), &ragged).is_empty());
        assert_eq!(client.state(), State::Requesting);
    }

    #[test]
    fn discover_is_retransmitted_with_backoff_and_then_the_client_gives_up() {
        let mut client = client();
        client.start(at(0));
        assert_eq!(client.next_deadline(), Some(at(4)));
        assert!(client.tick(at(3)).is_empty());
        assert_eq!(sent(&client.tick(at(4))), [(MessageType::Discover, Ipv4Addr::BROADCAST)]);
        // 4, then +8, +16, +32 and +64 s
        assert_eq!(client.next_deadline(), Some(at(12)));
        client.tick(at(12));
        client.tick(at(28));
        client.tick(at(60));
        assert_eq!(client.next_deadline(), Some(at(124)));
        let actions = client.tick(at(124));
        assert_eq!(actions[0], Action::GaveUp);
        assert_eq!(sent(&actions), [(MessageType::Discover, Ipv4Addr::BROADCAST)]);
        // a fresh transaction
        assert_eq!(xid_of(&actions), 1001);
        assert_eq!(client.next_deadline(), Some(at(128)));
    }

    #[test]
    fn a_nak_to_the_request_restarts_after_ten_seconds() {
        let mut client = client();
        client.start(at(0));
        client.receive(at(1), &server_message(MessageType::Offer, 1000, 1000, None, None));
        assert!(client.receive(at(2), &server_message(MessageType::Nak, 1000, 0, None, None)).is_empty());
        assert_eq!(client.state(), State::Init);
        assert!(client.tick(at(11)).is_empty());
        let actions = client.tick(at(12));
        assert_eq!(sent(&actions), [(MessageType::Discover, Ipv4Addr::BROADCAST)]);
        assert_eq!(xid_of(&actions), 1001);
        assert_eq!(client.state(), State::Selecting);
    }

    #[test]
    fn the_request_is_retransmitted_when_the_server_is_silent() {
        let mut client = client();
        client.start(at(0));
        client.receive(at(1), &server_message(MessageType::Offer, 1000, 1000, None, None));
        assert_eq!(sent(&client.tick(at(5))), [(MessageType::Request, Ipv4Addr::BROADCAST)]);
    }

    #[test]
    fn at_t1_the_client_renews_with_the_server_and_an_ack_starts_a_new_lease() {
        let mut client = bound_client();
        assert!(client.tick(at(499)).is_empty());
        let actions = client.tick(at(500));
        assert_eq!(sent(&actions), [(MessageType::Request, SERVER)]);
        let Action::Send { message, .. } = &actions[0] else { unreachable!() };
        assert_eq!(message.client_address, ADDRESS);
        assert_eq!((message.options.requested_address, message.options.server_id), (None, None));
        assert!(!message.broadcast);
        assert_eq!(client.state(), State::Renewing);
        // half the 375 s left until T2 (875 s)
        assert_eq!(client.next_deadline(), Some(Duration::from_millis(687_500)));

        let ack = client.receive(at(501), &server_message(MessageType::Ack, message.xid, 2000, None, None));
        assert!(matches!(&ack[..], [Action::Bound(lease)] if lease.duration == Some(at(2000))));
        assert_eq!(client.state(), State::Bound);
        // the new lease counts from the renewal REQUEST at 500 s: T1 = 500 + 1000
        assert_eq!(client.next_deadline(), Some(at(1500)));
    }

    #[test]
    fn without_an_answer_the_client_rebinds_at_t2_to_everyone() {
        let mut client = bound_client();
        client.tick(at(500));
        let actions = client.tick(at(875));
        assert_eq!(sent(&actions), [(MessageType::Request, Ipv4Addr::BROADCAST)]);
        assert_eq!(client.state(), State::Rebinding);
        // half of the 125 s left to expiry
        assert_eq!(client.next_deadline(), Some(Duration::from_millis(937_500)));
        let ack = client.receive(at(876), &server_message(MessageType::Ack, xid_of(&actions), 1000, None, None));
        assert!(matches!(&ack[..], [Action::Bound(_)]));
        assert_eq!(client.state(), State::Bound);
    }

    #[test]
    fn a_nak_while_renewing_drops_the_lease() {
        let mut client = bound_client();
        let actions = client.tick(at(500));
        let lost = client.receive(at(501), &server_message(MessageType::Nak, xid_of(&actions), 0, None, None));
        assert!(matches!(&lost[..], [Action::Lost(lease)] if lease.address == ADDRESS));
        assert_eq!(client.state(), State::Init);
        assert_eq!(client.lease(), None);
    }

    #[test]
    fn when_the_lease_expires_the_address_is_lost_and_discovery_begins() {
        let mut client = bound_client();
        client.tick(at(500));
        client.tick(at(875));
        let actions = client.tick(at(1000));
        assert!(matches!(&actions[0], Action::Lost(lease) if lease.address == ADDRESS));
        assert_eq!(sent(&actions), [(MessageType::Discover, Ipv4Addr::BROADCAST)]);
        assert_eq!(client.state(), State::Selecting);
    }

    #[test]
    fn a_tick_long_after_the_lease_expired_loses_it_at_once() {
        let mut client = bound_client();
        let actions = client.tick(at(5000));
        assert!(matches!(&actions[0], Action::Lost(_)));
        assert_eq!(client.state(), State::Selecting);
    }

    #[test]
    fn the_server_may_set_t1_and_t2() {
        let mut client = client();
        client.start(at(0));
        client.receive(at(0), &server_message(MessageType::Offer, 1000, 1000, None, None));
        client.receive(at(0), &server_message(MessageType::Ack, 1000, 1000, Some(100), Some(200)));
        assert_eq!(client.next_deadline(), Some(at(100)));
        client.tick(at(100));
        assert_eq!(client.next_deadline(), Some(at(100 + 60)));
        client.tick(at(200));
        assert_eq!(client.state(), State::Rebinding);
    }

    #[test]
    fn an_infinite_lease_never_renews() {
        let mut client = client();
        client.start(at(0));
        client.receive(at(0), &server_message(MessageType::Offer, 1000, u32::MAX, None, None));
        let bound = client.receive(at(0), &server_message(MessageType::Ack, 1000, u32::MAX, None, None));
        assert!(matches!(&bound[..], [Action::Bound(lease)] if lease.duration.is_none()));
        assert_eq!(client.next_deadline(), None);
        assert!(client.tick(at(10_000_000)).is_empty());
    }

    #[test]
    fn release_tells_the_server_and_forgets_the_lease() {
        let mut client = bound_client();
        let actions = client.release();
        assert_eq!(sent(&actions), [(MessageType::Release, SERVER)]);
        let Action::Send { message, .. } = &actions[0] else { unreachable!() };
        assert_eq!((message.client_address, message.options.server_id), (ADDRESS, Some(SERVER)));
        assert!(matches!(&actions[1], Action::Lost(_)));
        assert_eq!((client.state(), client.lease(), client.next_deadline()), (State::Init, None, None));
    }
}
