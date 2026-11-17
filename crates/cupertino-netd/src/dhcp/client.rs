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
const RETRANSMIT_DELAYS: [Duration; 5] = [
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
    Duration::from_secs(64),
];

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
    Send {
        message: Message,
        destination: Ipv4Addr,
    },
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
            State::Bound => self
                .binding
                .as_ref()
                .and_then(|binding| [binding.t1, binding.expires].into_iter().flatten().min()),
            State::Renewing => self.binding.as_ref().and_then(|binding| {
                [self.timer, binding.t2, binding.expires]
                    .into_iter()
                    .flatten()
                    .min()
            }),
            State::Rebinding => self
                .binding
                .as_ref()
                .and_then(|binding| [self.timer, binding.expires].into_iter().flatten().min()),
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
                State::Bound | State::Renewing | State::Rebinding => {
                    actions.extend(self.tick_bound(now))
                }
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
            (State::Requesting | State::Renewing | State::Rebinding, MessageType::Ack) => {
                self.on_ack(message)
            }
            (State::Requesting, MessageType::Nak) => {
                self.enter_init(now + NAK_HOLDOFF);
                Vec::new()
            }
            (State::Renewing | State::Rebinding, MessageType::Nak) => {
                let lost = self
                    .binding
                    .take()
                    .map(|binding| Action::Lost(binding.lease));
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
            actions.push(Action::Send {
                message,
                destination: binding.lease.server,
            });
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
        message.seconds =
            u16::try_from(now.saturating_sub(self.began).as_secs()).unwrap_or(u16::MAX);
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
        Action::Send {
            message,
            destination: Ipv4Addr::BROADCAST,
        }
    }

    fn send_selecting_request(&self, now: Timestamp) -> Action {
        let (server, address) = self
            .offer
            .expect("a request in REQUESTING follows an offer");
        let mut message = self.message(MessageType::Request, now);
        message.broadcast = true;
        message.options.requested_address = Some(address);
        message.options.server_id = Some(server);
        message.options.parameter_request_list = PARAMETER_REQUEST_LIST.to_vec();
        Action::Send {
            message,
            destination: Ipv4Addr::BROADCAST,
        }
    }

    /// RENEWING sends to the server, REBINDING to everyone, both from the leased address with
    /// `ciaddr` set and without the requested-address and server options (RFC 2131, 4.3.2).
    fn send_renewal_request(&self, now: Timestamp, destination: Ipv4Addr) -> Action {
        let lease = &self.binding.as_ref().expect("renewal needs a lease").lease;
        let mut message = self.message(MessageType::Request, now);
        message.client_address = lease.address;
        message.options.parameter_request_list = PARAMETER_REQUEST_LIST.to_vec();
        Action::Send {
            message,
            destination,
        }
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
        let prefix_len = u8::try_from(mask.leading_ones())
            .ok()
            .filter(|ones| mask.checked_shl(u32::from(*ones)).unwrap_or(0) == 0)?;
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
            t1: lease.duration.map(|duration| {
                start
                    + timer_after(
                        duration,
                        renewal,
                        DEFAULT_T1_NUMERATOR,
                        DEFAULT_T1_DENOMINATOR,
                    )
            }),
            t2: lease.duration.map(|duration| {
                start
                    + timer_after(
                        duration,
                        rebinding,
                        DEFAULT_T2_NUMERATOR,
                        DEFAULT_T2_DENOMINATOR,
                    )
            }),
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
            let send = if self.state == State::Selecting {
                self.send_discover(now)
            } else {
                self.send_selecting_request(now)
            };
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
            let destination = if self.state == State::Renewing {
                binding.lease.server
            } else {
                Ipv4Addr::BROADCAST
            };
            return self.renewal_attempt(now, destination, &binding, false);
        }
        Vec::new()
    }

    /// Sends a renewal REQUEST and sets the next retransmission: half the time left until T2
    /// (RENEWING) or until expiry (REBINDING), at least 60 s (RFC 2131, 4.4.5). A new request of
    /// a state starts a new transaction.
    fn renewal_attempt(
        &mut self,
        now: Timestamp,
        destination: Ipv4Addr,
        binding: &Binding,
        first: bool,
    ) -> Vec<Action> {
        if first {
            self.new_transaction();
            self.request_sent = now;
        }
        let until = if self.state == State::Renewing {
            binding.t2.or(binding.expires)
        } else {
            binding.expires
        };
        let remaining = until.map_or(MIN_RENEW_RETRY, |until| until.saturating_sub(now));
        self.timer = Some(now + (remaining / 2).max(MIN_RENEW_RETRY));
        vec![self.send_renewal_request(now, destination)]
    }
}

/// The time from the start of the lease to a timer: the server's value, else `numerator /
/// denominator` of the lease.
fn timer_after(
    lease: Duration,
    server_value: Option<u32>,
    numerator: u32,
    denominator: u32,
) -> Duration {
    server_value.map_or_else(
        || lease * numerator / denominator,
        |seconds| Duration::from_secs(u64::from(seconds)),
    )
}
