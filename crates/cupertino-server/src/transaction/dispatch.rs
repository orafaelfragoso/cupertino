use super::*;

impl GlobalDispatch<CupertinoTransactionManagerV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoTransactionManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }

    fn can_view(client: Client, _: &()) -> bool {
        client
            .get_data::<ClientState>()
            .is_some_and(|s| s.component.is_some())
    }
}

impl Dispatch<CupertinoTransactionManagerV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoTransactionManagerV1,
        request: cupertino_transaction_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        let now = Instant::now();
        let Some(conn) = client.get_data::<ClientState>().and_then(|s| s.component) else {
            resource.post_error(
                ManagerError::Permission,
                "only component connections may use transactions",
            );
            return;
        };
        let client_id = client.id();

        match request {
            cupertino_transaction_manager_v1::Request::Destroy => {}
            cupertino_transaction_manager_v1::Request::Create { id, serial, output } => {
                let key = TransactionKey {
                    component: conn.name,
                    instance: conn.instance,
                    serial,
                };
                // Check if serial already exists (unless it is an orphan join waiting for us).
                if let Some(&existing_id) = state.transactions.by_key.get(&key) {
                    let is_orphan_join = state
                        .transactions
                        .transactions
                        .get(&existing_id)
                        .is_some_and(|tx| tx.orphan_join_until.is_some());
                    if !is_orphan_join {
                        let is_active = state
                            .transactions
                            .transactions
                            .get(&existing_id)
                            .is_some_and(|tx| !matches!(tx.phase, TxPhase::Ended { .. }));
                        if is_active {
                            resource.post_error(
                                ManagerError::Serial,
                                format!("transaction serial {serial} is already active"),
                            );
                            return;
                        }
                    } else {
                        // Attach creator to the waiting orphan join transaction!
                        if state.transactions.active_tx_count_for(&client_id)
                            >= MAX_ACTIVE_TRANSACTIONS
                        {
                            resource.post_error(
                                ManagerError::TooMany,
                                "connection already has 16 active transactions",
                            );
                            return;
                        }
                        let output_id = Output::from_resource(&output)
                            .and_then(|o| state.output_by_wl(&o).map(|e| e.id))
                            .or_else(|| state.primary_output().map(|o| o.id))
                            .unwrap_or(OutputId(0));
                        let pid = state.transactions.next_participant_id;
                        state.transactions.next_participant_id += 1;
                        let obj = init.init(
                            id,
                            TransactionObjectData {
                                tx_id: existing_id,
                                participant_id: pid,
                                is_creator: true,
                            },
                        );
                        if let Some(tx) = state.transactions.transactions.get_mut(&existing_id) {
                            tx.orphan_join_until = None;
                            tx.output = output_id;
                            tx.participants.push(Participant {
                                id: pid,
                                component: conn.name,
                                client_id,
                                is_creator: true,
                                resource: Some(obj),
                                created_at: now,
                                submitted_at: None,
                                staged: Vec::new(),
                                window_changes: Vec::new(),
                                ready: false,
                                ready_since: None,
                                applied: false,
                                withdrawn: false,
                            });
                        }
                        return;
                    }
                }
                if state.transactions.active_tx_count_for(&client_id) >= MAX_ACTIVE_TRANSACTIONS {
                    resource.post_error(
                        ManagerError::TooMany,
                        "connection already has 16 active transactions",
                    );
                    return;
                }
                let output_id = Output::from_resource(&output)
                    .and_then(|o| state.output_by_wl(&o).map(|e| e.id))
                    .or_else(|| state.primary_output().map(|o| o.id))
                    .unwrap_or(OutputId(0));
                let tx_id = TxId(state.transactions.next_tx_id);
                state.transactions.next_tx_id += 1;
                let pid = state.transactions.next_participant_id;
                state.transactions.next_participant_id += 1;
                let obj = init.init(
                    id,
                    TransactionObjectData {
                        tx_id,
                        participant_id: pid,
                        is_creator: true,
                    },
                );
                state.transactions.by_key.insert(key, tx_id);
                state.transactions.transactions.insert(
                    tx_id,
                    TransactionRecord {
                        id: tx_id,
                        key,
                        output: output_id,
                        orphan_join_until: None,
                        expected: Vec::new(),
                        deadline_ms: None,
                        ready_at: None,
                        phase: TxPhase::Pending,
                        gesture: None,
                        participants: vec![Participant {
                            id: pid,
                            component: conn.name,
                            client_id,
                            is_creator: true,
                            resource: Some(obj),
                            created_at: now,
                            submitted_at: None,
                            staged: Vec::new(),
                            window_changes: Vec::new(),
                            ready: false,
                            ready_since: None,
                            applied: false,
                            withdrawn: false,
                        }],
                    },
                );
            }
            cupertino_transaction_manager_v1::Request::Join {
                id,
                component,
                instance,
                serial,
                output,
            } => {
                let Some(spec) = component_spec(&component) else {
                    resource.post_error(
                        ManagerError::Component,
                        format!("unknown component name: {component}"),
                    );
                    return;
                };
                if state.transactions.active_tx_count_for(&client_id) >= MAX_ACTIVE_TRANSACTIONS {
                    resource.post_error(
                        ManagerError::TooMany,
                        "connection already has 16 active transactions",
                    );
                    return;
                }
                let key = TransactionKey {
                    component: spec.name,
                    instance,
                    serial,
                };
                let pid = state.transactions.next_participant_id;
                state.transactions.next_participant_id += 1;
                if let Some(&existing_id) = state.transactions.by_key.get(&key) {
                    let obj = init.init(
                        id,
                        TransactionObjectData {
                            tx_id: existing_id,
                            participant_id: pid,
                            is_creator: false,
                        },
                    );
                    if let Some(tx) = state.transactions.transactions.get_mut(&existing_id) {
                        if let TxPhase::Ended { outcome, .. } = tx.phase {
                            obj.finished(outcome);
                            return;
                        }
                        tx.participants.push(Participant {
                            id: pid,
                            component: conn.name,
                            client_id,
                            is_creator: false,
                            resource: Some(obj),
                            created_at: now,
                            submitted_at: None,
                            staged: Vec::new(),
                            window_changes: Vec::new(),
                            ready: false,
                            ready_since: None,
                            applied: false,
                            withdrawn: false,
                        });
                    }
                    return;
                }
                // No transaction yet: wait up to JOIN_WAIT_MS for creator's `create`.
                let output_id = Output::from_resource(&output)
                    .and_then(|o| state.output_by_wl(&o).map(|e| e.id))
                    .or_else(|| state.primary_output().map(|o| o.id))
                    .unwrap_or(OutputId(0));
                let tx_id = TxId(state.transactions.next_tx_id);
                state.transactions.next_tx_id += 1;
                let obj = init.init(
                    id,
                    TransactionObjectData {
                        tx_id,
                        participant_id: pid,
                        is_creator: false,
                    },
                );
                state.transactions.by_key.insert(key, tx_id);
                state.transactions.transactions.insert(
                    tx_id,
                    TransactionRecord {
                        id: tx_id,
                        key,
                        output: output_id,
                        orphan_join_until: Some(now + Duration::from_millis(JOIN_WAIT_MS.into())),
                        expected: Vec::new(),
                        deadline_ms: None,
                        ready_at: None,
                        phase: TxPhase::Pending,
                        gesture: None,
                        participants: vec![Participant {
                            id: pid,
                            component: conn.name,
                            client_id,
                            is_creator: false,
                            resource: Some(obj),
                            created_at: now,
                            submitted_at: None,
                            staged: Vec::new(),
                            window_changes: Vec::new(),
                            ready: false,
                            ready_since: None,
                            applied: false,
                            withdrawn: false,
                        }],
                    },
                );
            }
            cupertino_transaction_manager_v1::Request::GetHole { id, surface } => {
                if !authorized(client, ShellRole::Windows) {
                    resource.post_error(
                        ManagerError::Permission,
                        "get_hole requires the windows role",
                    );
                    return;
                }
                if !surface.is_alive() || surface.client().as_ref() != Some(client) {
                    resource.post_error(
                        ManagerError::InvalidSurface,
                        "surface does not belong to this client or is gone",
                    );
                    return;
                }
                if get_role(&surface) != Some(LAYER_ROLE)
                    || state
                        .transactions
                        .hole_by_surface
                        .contains_key(&surface.id())
                {
                    resource.post_error(
                        ManagerError::Role,
                        "surface is not a cupertino_layer_v1 or already has a hole",
                    );
                    return;
                }
                if state.transactions.hole_count_for(&client_id) >= MAX_HOLES_PER_CONNECTION {
                    resource
                        .post_error(ManagerError::TooMany, "connection already holds 256 holes");
                    return;
                }
                let hid = HoleId(state.transactions.next_hole_id);
                state.transactions.next_hole_id += 1;
                let obj = init.init(id, HoleObjectData { hole_id: hid });
                state.transactions.hole_by_surface.insert(surface.id(), hid);
                state.transactions.holes.insert(
                    hid,
                    HoleRecord {
                        id: hid.0,
                        client_id,
                        layer_surface_id: surface.id(),
                        resource: obj,
                        pending_source: HoleSource::None,
                        committed_source: HoleSource::None,
                        pending_claim: false,
                        committed_claim: false,
                        queued: VecDeque::new(),
                        closing: None,
                        component: conn.name,
                        shown: Cell::new(None),
                        held: None,
                    },
                );
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoTransactionV1, TransactionObjectData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoTransactionV1,
        request: cupertino_transaction_v1::Request,
        data: &TransactionObjectData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let now = Instant::now();
        let client_id = client.id();

        let is_submitted = state
            .transactions
            .transactions
            .get(&data.tx_id)
            .and_then(|tx| tx.participants.iter().find(|p| p.id == data.participant_id))
            .is_some_and(|p| p.submitted_at.is_some());

        match request {
            cupertino_transaction_v1::Request::Destroy => {
                let mut flush_changes = Vec::new();
                if let Some(tx) = state.transactions.transactions.get_mut(&data.tx_id) {
                    if let Some(p) = tx
                        .participants
                        .iter_mut()
                        .find(|p| p.id == data.participant_id)
                    {
                        p.resource = None;
                        if p.submitted_at.is_none() {
                            p.withdrawn = true;
                            p.leave_staged(
                                data.tx_id,
                                &mut state.transactions.staged_surface_owner,
                            );
                            flush_changes = std::mem::take(&mut p.window_changes);
                            if state
                                .transactions
                                .open_brackets
                                .get(&client_id)
                                .is_some_and(|&(tid, pid)| {
                                    tid == data.tx_id && pid == data.participant_id
                                })
                            {
                                state.transactions.open_brackets.remove(&client_id);
                            }
                        }
                    }
                    if data.is_creator && tx.gesture.is_some() {
                        if let Some(g) = &mut tx.gesture {
                            g.reverse_gesture = None;
                        }
                        match tx.phase {
                            // the binding ends with the creator's object: an armed transaction is cancelled for all
                            TxPhase::Armed => {
                                flush_changes
                                    .extend(state.transactions.end_cancelled(data.tx_id, now));
                            }
                            TxPhase::RestingCompleted => {
                                tx.phase = TxPhase::Ended {
                                    ended_at: now,
                                    outcome: WireOutcome::Cancelled,
                                };
                            }
                            // a gesture under way settles back to its start
                            TxPhase::GestureTracking | TxPhase::GestureSettling => {
                                let model_now = state.now();
                                state.transactions.settle_back(data.tx_id, model_now);
                            }
                            _ => {}
                        }
                    }
                }
                for change in flush_changes {
                    state.apply_queued_window_change(change);
                }
                // a creator withdrawn before it submitted leaves nothing to start the transaction: it ends as cancelled
                state.end_abandoned_transactions(now);
                // a part withdrawn or cancelled before it applied: its kept commits apply now, as ordinary commits
                state.release_left_holds();
            }
            cupertino_transaction_v1::Request::Expect { component } => {
                if !data.is_creator {
                    resource.post_error(TxError::Creator, "only the creator may call expect");
                    return;
                }
                if is_submitted {
                    resource.post_error(TxError::Submitted, "expect after submit");
                    return;
                }
                let Some(spec) = component_spec(&component) else {
                    resource.post_error(
                        TxError::Component,
                        format!("unknown component name: {component}"),
                    );
                    return;
                };
                if let Some(tx) = state.transactions.transactions.get_mut(&data.tx_id) {
                    if tx.expected.len() >= MAX_EXPECTED_COMPONENTS
                        && !tx.expected.contains(&spec.name)
                    {
                        resource.post_error(TxError::TooMany, "at most 16 expected components");
                        return;
                    }
                    if !tx.expected.contains(&spec.name) {
                        tx.expected.push(spec.name);
                    }
                }
            }
            cupertino_transaction_v1::Request::SetDeadline { milliseconds } => {
                if !data.is_creator {
                    resource.post_error(TxError::Creator, "only the creator may set_deadline");
                    return;
                }
                if is_submitted {
                    resource.post_error(TxError::Submitted, "set_deadline after submit");
                    return;
                }
                if let Some(tx) = state.transactions.transactions.get_mut(&data.tx_id) {
                    tx.deadline_ms = Some(milliseconds.min(MAX_DEADLINE_MS));
                }
            }
            cupertino_transaction_v1::Request::Stage { surface } => {
                if is_submitted {
                    resource.post_error(TxError::Submitted, "stage after submit");
                    return;
                }
                if !surface.is_alive() || surface.client().as_ref() != Some(client) {
                    resource.post_error(
                        TxError::InvalidSurface,
                        "surface does not belong to this client or is gone",
                    );
                    return;
                }
                let role = get_role(&surface);
                if role.is_none() || role == Some(LAYER_ROLE) {
                    resource.post_error(
                        TxError::InvalidSurface,
                        "staged surface must have a non-layer role",
                    );
                    return;
                }
                if state
                    .transactions
                    .staged_surface_owner
                    .contains_key(&surface.id())
                {
                    resource.post_error(
                        TxError::Staged,
                        "surface is already staged in an unstarted transaction",
                    );
                    return;
                }
                if let Some(tx) = state.transactions.transactions.get_mut(&data.tx_id) {
                    if let Some(p) = tx
                        .participants
                        .iter_mut()
                        .find(|p| p.id == data.participant_id)
                    {
                        if p.staged.len() >= MAX_STAGED_SURFACES {
                            resource.post_error(TxError::TooMany, "at most 32 staged surfaces");
                            return;
                        }
                        let sid = surface.id();
                        p.staged.push(StagedSurface {
                            surface_id: sid.clone(),
                            committed: false,
                        });
                        state
                            .transactions
                            .staged_surface_owner
                            .insert(sid, (data.tx_id, data.participant_id));
                    }
                }
            }
            cupertino_transaction_v1::Request::BeginWindowChanges => {
                if is_submitted {
                    resource.post_error(TxError::Submitted, "begin_window_changes after submit");
                    return;
                }
                if state.transactions.open_brackets.contains_key(&client_id) {
                    resource.post_error(
                        TxError::Bracket,
                        "connection already has an open window-change bracket",
                    );
                    return;
                }
                state
                    .transactions
                    .open_brackets
                    .insert(client_id, (data.tx_id, data.participant_id));
            }
            cupertino_transaction_v1::Request::EndWindowChanges => {
                let owns = state
                    .transactions
                    .open_brackets
                    .get(&client_id)
                    .is_some_and(|&(tid, pid)| tid == data.tx_id && pid == data.participant_id);
                if !owns {
                    resource.post_error(
                        TxError::Bracket,
                        "end_window_changes without matching open bracket",
                    );
                    return;
                }
                state.transactions.open_brackets.remove(&client_id);
            }
            cupertino_transaction_v1::Request::BindGesture {
                gesture,
                response,
                damping,
            } => {
                if !data.is_creator {
                    resource.post_error(TxError::Creator, "only the creator may bind_gesture");
                    return;
                }
                if is_submitted {
                    resource.post_error(TxError::Submitted, "bind_gesture after submit");
                    return;
                }
                if !authorized(client, ShellRole::Overview) {
                    resource.post_error(
                        TxError::Permission,
                        "bind_gesture requires the overview role",
                    );
                    return;
                }
                let WEnum::Value(gesture) = gesture else {
                    resource.post_error(TxError::InvalidGesture, "invalid gesture enum");
                    return;
                };
                if !(SPRING_RESPONSE.0..=SPRING_RESPONSE.1).contains(&response)
                    || !(SPRING_DAMPING.0..=SPRING_DAMPING.1).contains(&damping)
                {
                    resource.post_error(TxError::InvalidSpring, "spring parameters out of range");
                    return;
                }
                state.transactions.next_seq += 1;
                let seq = state.transactions.next_seq;
                if let Some(tx) = state.transactions.transactions.get_mut(&data.tx_id) {
                    tx.gesture = Some(GestureBinding {
                        gesture,
                        reverse_gesture: None,
                        response,
                        damping,
                        armed_seq: seq,
                        completed_seq: None,
                        progress: GestureProgress::new(),
                        settle: None,
                        reversing: false,
                    });
                }
            }
            cupertino_transaction_v1::Request::BindReverseGesture { gesture } => {
                if !data.is_creator {
                    resource.post_error(
                        TxError::Creator,
                        "only the creator may bind_reverse_gesture",
                    );
                    return;
                }
                if is_submitted {
                    resource.post_error(TxError::Submitted, "bind_reverse_gesture after submit");
                    return;
                }
                if !authorized(client, ShellRole::Overview) {
                    resource.post_error(
                        TxError::Permission,
                        "bind_reverse_gesture requires the overview role",
                    );
                    return;
                }
                let WEnum::Value(gesture) = gesture else {
                    resource.post_error(TxError::InvalidGesture, "invalid reverse gesture enum");
                    return;
                };
                let bound = state
                    .transactions
                    .transactions
                    .get_mut(&data.tx_id)
                    .and_then(|tx| tx.gesture.as_mut());
                match bound {
                    Some(g) => g.reverse_gesture = Some(gesture),
                    None => resource.post_error(
                        TxError::InvalidGesture,
                        "bind_reverse_gesture requires bind_gesture first",
                    ),
                }
            }
            cupertino_transaction_v1::Request::Submit => {
                let open_here = state
                    .transactions
                    .open_brackets
                    .get(&client_id)
                    .is_some_and(|&(tid, pid)| tid == data.tx_id && pid == data.participant_id);
                if open_here {
                    resource.post_error(TxError::Bracket, "submit inside an open bracket");
                    return;
                }
                if let Some(tx) = state.transactions.transactions.get_mut(&data.tx_id) {
                    if let Some(p) = tx
                        .participants
                        .iter_mut()
                        .find(|p| p.id == data.participant_id)
                    {
                        if p.submitted_at.is_none() {
                            p.submitted_at = Some(now);
                            if p.all_staged_committed() {
                                p.mark_ready(now);
                            }
                        }
                    }
                }
                state.tick_transactions(now);
            }
            cupertino_transaction_v1::Request::Cancel => {
                let mut flush_changes = Vec::new();
                if !data.is_creator {
                    // Non-creator cancel withdraws only this participant.
                    if let Some(tx) = state.transactions.transactions.get_mut(&data.tx_id) {
                        if let Some(p) = tx
                            .participants
                            .iter_mut()
                            .find(|p| p.id == data.participant_id)
                        {
                            p.withdrawn = true;
                            p.leave_staged(
                                data.tx_id,
                                &mut state.transactions.staged_surface_owner,
                            );
                            flush_changes = std::mem::take(&mut p.window_changes);
                        }
                    }
                    if state
                        .transactions
                        .open_brackets
                        .get(&client_id)
                        .is_some_and(|&(tid, pid)| tid == data.tx_id && pid == data.participant_id)
                    {
                        state.transactions.open_brackets.remove(&client_id);
                    }
                } else {
                    // Creator cancel ends the transaction for every participant; a gesture under way settles back to its
                    // start first
                    let model_now = state.now();
                    if !state.transactions.settle_back(data.tx_id, model_now) {
                        flush_changes = state.transactions.end_cancelled(data.tx_id, now);
                    }
                }
                for change in flush_changes {
                    state.apply_queued_window_change(change);
                }
                // a part withdrawn or cancelled before it applied: its kept commits apply now, as ordinary commits
                state.release_left_holds();
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHoleV1, HoleObjectData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoHoleV1,
        request: cupertino_hole_v1::Request,
        data: &HoleObjectData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_hole_v1::Request::Destroy => {
                if let Some(hole) = state.transactions.holes.remove(&data.hole_id) {
                    state
                        .transactions
                        .hole_by_surface
                        .remove(&hole.layer_surface_id);
                    for stack in state.transactions.window_claims.values_mut() {
                        stack.retain(|&id| id != data.hole_id);
                    }
                    state
                        .transactions
                        .window_claims
                        .retain(|_, stack| !stack.is_empty());
                }
            }
            cupertino_hole_v1::Request::SetSource { source, id } => {
                let WEnum::Value(source) = source else {
                    resource.post_error(HoleError::InvalidSource, "invalid hole source");
                    return;
                };
                if source == WireSource::Workspace && !authorized(client, ShellRole::Overview) {
                    resource.post_error(
                        HoleError::Permission,
                        "workspace hole source requires the overview role",
                    );
                    return;
                }
                let parsed = match source {
                    WireSource::None => HoleSource::None,
                    WireSource::Window => HoleSource::Window(WindowId(u64::from(id))),
                    WireSource::Thumbnail => HoleSource::Thumbnail(WindowId(u64::from(id))),
                    WireSource::Workspace => HoleSource::Workspace(WorkspaceId(u64::from(id))),
                    _ => {
                        resource.post_error(HoleError::InvalidSource, "unknown hole source");
                        return;
                    }
                };
                if let Some(hole) = state.transactions.holes.get_mut(&data.hole_id) {
                    hole.pending_source = parsed;
                }
            }
            cupertino_hole_v1::Request::SetClaim { claim } => {
                if !authorized(client, ShellRole::Overview)
                    && !authorized(client, ShellRole::WindowManager)
                {
                    resource.post_error(
                        HoleError::Permission,
                        "set_claim requires overview or window_manager role",
                    );
                    return;
                }
                if let Some(hole) = state.transactions.holes.get_mut(&data.hole_id) {
                    hole.pending_claim = claim != 0;
                }
            }
            _ => {}
        }
    }
}
