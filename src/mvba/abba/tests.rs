use std::rc::Rc;
use std::{cell::RefCell, collections::BTreeMap};

use blsttc::{SecretKey, SecretKeySet, Signature};
use rand::thread_rng;

use super::{
    error::Error,
    message::{
        Action, MainVoteAction, MainVoteJustification, MainVoteValue, Message, PreVoteAction,
        PreVoteJustification, Value,
    },
    Abba,
};
use crate::mvba::hash::Hash32;
use crate::mvba::tag::{Domain, Tag};
use crate::mvba::{broadcaster::Broadcaster, bundle::Bundle, NodeId};

struct TestNet {
    sec_key_set: SecretKeySet,
    abba: Abba,
    proposal_digest: Hash32,
    proposal_sig: Signature,
    broadcaster: Rc<RefCell<Broadcaster>>,
}

impl TestNet {
    const PARTY_X: NodeId = 0;
    const PARTY_Y: NodeId = 1;
    const PARTY_B: NodeId = 2;
    const PARTY_S: NodeId = 3;

    // There are 4 parties: X, Y, B, S (B is Byzantine and S is Slow)
    // The ABBA test instance created for party `i`, `Tag` set to `test-domain.j.0`
    pub fn new(i: NodeId, j: NodeId) -> Self {
        let tag = Tag::new(Domain::new("test-domain", 0), j);

        let mut rng = thread_rng();
        let sec_key_set = SecretKeySet::random(2, &mut rng);
        let sec_key_share = sec_key_set.secret_key_share(i);

        let proposal_digest = Hash32::calculate("test-data".as_bytes());
        let sign_bytes = crate::mvba::vcbc::c_ready_bytes_to_sign(&tag, &proposal_digest).unwrap();
        let proposal_sig = sec_key_set.secret_key().sign(sign_bytes);

        let broadcaster = Rc::new(RefCell::new(Broadcaster::new(i)));
        let abba = Abba::new(
            tag,
            i,
            sec_key_set.public_keys(),
            sec_key_share,
            broadcaster.clone(),
        );

        Self {
            sec_key_set,
            abba,
            proposal_digest,
            proposal_sig,
            broadcaster,
        }
    }

    pub fn make_pre_vote_msg(
        &self,
        round: usize,
        value: Value,
        justification: &PreVoteJustification,
        peer_id: &NodeId,
    ) -> Message {
        let sign_bytes = self.abba.pre_vote_bytes_to_sign(round, &value).unwrap();
        let sig_share = self.sec_key_set.secret_key_share(peer_id).sign(sign_bytes);
        Message {
            tag: self.abba.tag.clone(),
            action: Action::PreVote(PreVoteAction {
                round,
                value,
                justification: justification.clone(),
                sig_share,
            }),
        }
    }

    pub fn make_main_vote_msg(
        &self,
        round: usize,
        value: MainVoteValue,
        justification: &MainVoteJustification,
        peer_id: &NodeId,
    ) -> Message {
        let sign_bytes = self.abba.main_vote_bytes_to_sign(round, &value).unwrap();
        let sig_share = self.sec_key_set.secret_key_share(peer_id).sign(sign_bytes);
        Message {
            tag: self.abba.tag.clone(),
            action: Action::MainVote(MainVoteAction {
                round,
                value,
                justification: justification.clone(),
                sig_share,
            }),
        }
    }

    pub fn is_broadcasted(&self, msg: &Message) -> bool {
        self.broadcaster
            .borrow()
            .has_gossip_message(&bincode::serialize(msg).unwrap())
    }
}

#[test]
fn test_round_votes() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    t.abba
        .pre_vote_one(t.proposal_digest, t.proposal_sig)
        .unwrap();

    assert!(t.abba.get_pre_votes_by_round(1).unwrap().len() == 1);
    assert!(t.abba.get_pre_votes_by_round(2).is_none());
    assert!(t.abba.get_main_votes_by_round(2).is_none());
    assert!(t.abba.get_main_votes_by_round(2).is_none());
}

#[test]
fn test_should_publish_pre_vote_message() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    t.abba
        .pre_vote_one(t.proposal_digest, t.proposal_sig.clone())
        .unwrap();

    let just = PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());
    let pre_vote_x = t.make_pre_vote_msg(1, Value::One, &just, &TestNet::PARTY_X);
    assert!(t.is_broadcasted(&pre_vote_x));
}

#[test]
fn test_should_publish_main_vote_message() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    t.abba
        .pre_vote_one(t.proposal_digest, t.proposal_sig.clone())
        .unwrap();
    let just = PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());

    let pre_vote_y = t.make_pre_vote_msg(1, Value::One, &just, &TestNet::PARTY_Y);
    let pre_vote_s = t.make_pre_vote_msg(1, Value::One, &just, &TestNet::PARTY_S);

    t.abba
        .receive_message(TestNet::PARTY_Y, pre_vote_y)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_S, pre_vote_s)
        .unwrap();

    let sign_bytes = t.abba.pre_vote_bytes_to_sign(1, &Value::One).unwrap();
    let sig = t.sec_key_set.secret_key().sign(sign_bytes);
    let main_vote_just = MainVoteJustification::NoAbstain(sig);
    let main_vote_x =
        t.make_main_vote_msg(1, MainVoteValue::one(), &main_vote_just, &TestNet::PARTY_X);

    assert!(t.is_broadcasted(&main_vote_x));
}

#[test]
fn test_ignore_messages_with_wrong_domain() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    let just = PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());
    let mut pre_vote_x = t.make_pre_vote_msg(1, Value::One, &just, &TestNet::PARTY_B);
    pre_vote_x.tag.domain = Domain::new("another-domain", 0);

    let result = t.abba.receive_message(TestNet::PARTY_B, pre_vote_x);
    match result {
        Err(Error::InvalidMessage(msg)) => assert_eq!(
            msg,
            format!("invalid tag. expected: test-domain[0].{j}, got another-domain[0].{j}"),
        ),
        other => panic!("Expected invalid message, got: {other:?}"),
    }
}

#[test]
fn test_ignore_messages_with_wrong_proposer() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    let just = PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());
    let mut pre_vote_x = t.make_pre_vote_msg(1, Value::One, &just, &TestNet::PARTY_B);
    pre_vote_x.tag.proposer = TestNet::PARTY_B;

    let result = t.abba.receive_message(TestNet::PARTY_B, pre_vote_x);
    match result {
        Err(Error::InvalidMessage(msg)) => assert_eq!(
            msg,
            "invalid tag. expected: test-domain[0].0, got test-domain[0].2"
        ),
        res => panic!("Should not have accepted the message: {res:?}"),
    }
}

#[test]
fn test_absent_main_vote_round_one_invalid_justification() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_B;
    let mut t = TestNet::new(i, j);

    let sign_bytes =
        crate::mvba::vcbc::c_ready_bytes_to_sign(&t.abba.tag, &t.proposal_digest).unwrap();
    let invalid_sig = SecretKey::random().sign(sign_bytes);

    let just_0 = PreVoteJustification::FirstRoundZero;
    let just_1 = PreVoteJustification::WithValidity(t.proposal_digest, invalid_sig);
    let just = MainVoteJustification::Abstain(Box::new(just_0), Box::new(just_1));

    let main_vote_b = t.make_main_vote_msg(1, MainVoteValue::Abstain, &just, &TestNet::PARTY_B);

    let result = t.abba.receive_message(TestNet::PARTY_B, main_vote_b);
    assert!(matches!(result, Err(Error::InvalidMessage(msg))
        if msg == "invalid signature for the VCBC proposal"));
}

#[test]
fn test_pre_vote_invalid_sig_share() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    let just = PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());
    let invalid_sig_share = t
        .sec_key_set
        .secret_key_share(TestNet::PARTY_B)
        .sign("invalid-msg");
    let msg = Message {
        tag: t.abba.tag.clone(),
        action: Action::PreVote(PreVoteAction {
            round: 1,
            justification: just,
            value: Value::One,
            sig_share: invalid_sig_share,
        }),
    };

    let result = t.abba.receive_message(TestNet::PARTY_B, msg);
    assert!(matches!(result, Err(Error::InvalidMessage(msg))
        if msg == "invalid signature share"));
}

#[test]
fn test_double_pre_vote() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    let just_0 = PreVoteJustification::FirstRoundZero;
    let pre_vote_1 = t.make_pre_vote_msg(1, Value::Zero, &just_0, &TestNet::PARTY_B);

    let just_1 = PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());
    let pre_vote_2 = t.make_pre_vote_msg(1, Value::One, &just_1, &TestNet::PARTY_B);

    t.abba
        .receive_message(TestNet::PARTY_B, pre_vote_1.clone())
        .unwrap();

    // Repeating the message, should not return any error
    t.abba
        .receive_message(TestNet::PARTY_B, pre_vote_1)
        .unwrap();

    let result = t.abba.receive_message(TestNet::PARTY_B, pre_vote_2);
    assert!(matches!(result, Err(Error::InvalidMessage(msg))
        if msg == format!(
            "double pre-vote detected from {:?}", &TestNet::PARTY_B)));
}

#[test]
fn test_pre_vote_round_1_invalid_c_final_signature() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_B;
    let mut t = TestNet::new(i, j);

    let sign_bytes =
        crate::mvba::vcbc::c_ready_bytes_to_sign(&t.abba.tag, &t.proposal_digest).unwrap();
    let invalid_sig = SecretKey::random().sign(sign_bytes);

    let just = PreVoteJustification::WithValidity(t.proposal_digest, invalid_sig);
    let msg = t.make_pre_vote_msg(1, Value::One, &just, &TestNet::PARTY_B);

    let result = t.abba.receive_message(TestNet::PARTY_B, msg);
    assert!(matches!(result, Err(Error::InvalidMessage(msg))
    if msg == *"invalid signature for the VCBC proposal"));
}

#[test]
fn test_pre_vote_round_1_invalid_value_one() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    let just = PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());
    let msg = t.make_pre_vote_msg(1, Value::Zero, &just, &TestNet::PARTY_B);

    let result = t.abba.receive_message(TestNet::PARTY_B, msg);
    assert!(matches!(result, Err(Error::InvalidMessage(msg))
    if msg == "initial value should be one"));
}

#[test]
fn test_pre_vote_round_1_invalid_value_zero() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    let just = PreVoteJustification::FirstRoundZero;
    let msg = t.make_pre_vote_msg(1, Value::One, &just, &TestNet::PARTY_B);

    let result = t.abba.receive_message(TestNet::PARTY_B, msg);
    assert!(matches!(result, Err(Error::InvalidMessage(msg))
    if msg == "initial value should be zero"));
}

#[test]
fn test_prevent_double_pre_vote() {
    let i = TestNet::PARTY_S;
    let j = TestNet::PARTY_B;
    let mut t = TestNet::new(i, j);

    // -- round 1
    // First we don't have the proposal, so we pre-vote for zero
    t.abba.pre_vote_zero().unwrap();

    let round_1_pre_vote_s = t.make_pre_vote_msg(
        1,
        Value::Zero,
        &PreVoteJustification::FirstRoundZero,
        &TestNet::PARTY_S,
    );
    assert!(t.is_broadcasted(&round_1_pre_vote_s));

    let round_1_pre_vote_x = t.make_pre_vote_msg(
        1,
        Value::Zero,
        &PreVoteJustification::FirstRoundZero,
        &TestNet::PARTY_X,
    );

    let round_1_pre_vote_y = t.make_pre_vote_msg(
        1,
        Value::Zero,
        &PreVoteJustification::FirstRoundZero,
        &TestNet::PARTY_Y,
    );

    t.abba
        .receive_message(TestNet::PARTY_X, round_1_pre_vote_x)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_Y, round_1_pre_vote_y)
        .unwrap();

    let sign_bytes = t.abba.pre_vote_bytes_to_sign(1, &Value::Zero).unwrap();
    let round_1_just_sig = t.sec_key_set.secret_key().sign(sign_bytes);
    let round_1_main_vote_just_no_abstain =
        MainVoteJustification::NoAbstain(round_1_just_sig.clone());

    let round_1_main_vote_s = t.make_main_vote_msg(
        1,
        MainVoteValue::zero(),
        &round_1_main_vote_just_no_abstain,
        &TestNet::PARTY_S,
    );
    assert!(t.is_broadcasted(&round_1_main_vote_s));

    // Other parties, receive the proposal, therefore they main-vote for abstain
    let weak_validity_just =
        PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());
    let round_1_main_vote_just_abstain = MainVoteJustification::Abstain(
        Box::new(PreVoteJustification::FirstRoundZero),
        Box::new(weak_validity_just.clone()),
    );

    let round_1_main_vote_x = t.make_main_vote_msg(
        1,
        MainVoteValue::Abstain,
        &round_1_main_vote_just_abstain,
        &TestNet::PARTY_X,
    );
    let round_1_main_vote_y = t.make_main_vote_msg(
        1,
        MainVoteValue::Abstain,
        &round_1_main_vote_just_abstain,
        &TestNet::PARTY_Y,
    );

    t.abba
        .receive_message(TestNet::PARTY_X, round_1_main_vote_x)
        .unwrap();

    t.abba
        .receive_message(TestNet::PARTY_Y, round_1_main_vote_y)
        .unwrap();

    // -- round 2
    // Still we don't have the proposal, again pre-vote for zero
    let round_2_pre_vote_s = t.make_pre_vote_msg(
        2,
        Value::Zero,
        &PreVoteJustification::Hard(round_1_just_sig),
        &TestNet::PARTY_S,
    );
    assert!(t.is_broadcasted(&round_2_pre_vote_s));

    // We receive a pre-vote from other parties with the weak validity info
    assert!(t.abba.weak_validity.is_none());

    let round_2_pre_vote_x =
        t.make_pre_vote_msg(2, Value::One, &weak_validity_just, &TestNet::PARTY_X);

    t.abba
        .receive_message(TestNet::PARTY_X, round_2_pre_vote_x)
        .unwrap();

    // We should set the weak validity data
    assert!(t.abba.weak_validity.is_some());

    // receiving a main-vote for round 1 from the PARTY_B,
    // we should not double pre-vote here
    let round_1_main_vote_b = t.make_main_vote_msg(
        1,
        MainVoteValue::Abstain,
        &round_1_main_vote_just_abstain,
        &TestNet::PARTY_B,
    );

    assert!(t
        .abba
        .receive_message(TestNet::PARTY_B, round_1_main_vote_b)
        .is_ok());
}

#[test]
fn test_normal_case_one_round() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    t.abba
        .pre_vote_one(t.proposal_digest, t.proposal_sig.clone())
        .unwrap();

    let pre_vote_just =
        PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());
    let pre_vote_y = t.make_pre_vote_msg(1, Value::One, &pre_vote_just, &TestNet::PARTY_Y);
    let pre_vote_s = t.make_pre_vote_msg(1, Value::One, &pre_vote_just, &TestNet::PARTY_S);

    t.abba
        .receive_message(TestNet::PARTY_Y, pre_vote_y)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_S, pre_vote_s)
        .unwrap();

    let sign_bytes = t.abba.pre_vote_bytes_to_sign(1, &Value::One).unwrap();
    let sig = t.sec_key_set.secret_key().sign(sign_bytes);
    let main_vote_just = MainVoteJustification::NoAbstain(sig);
    let main_vote_y =
        t.make_main_vote_msg(1, MainVoteValue::one(), &main_vote_just, &TestNet::PARTY_Y);
    let main_vote_s =
        t.make_main_vote_msg(1, MainVoteValue::one(), &main_vote_just, &TestNet::PARTY_S);

    t.abba
        .receive_message(TestNet::PARTY_Y, main_vote_y)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_S, main_vote_s)
        .unwrap();

    assert_eq!(t.abba.decided_value.unwrap().value, Value::One);
}

#[test]
fn test_normal_case_zero() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_B;
    let mut t = TestNet::new(i, j);

    t.abba.pre_vote_zero().unwrap();

    let round_1_just_0 = PreVoteJustification::FirstRoundZero;

    let round_1_pre_vote_y =
        t.make_pre_vote_msg(1, Value::Zero, &round_1_just_0, &TestNet::PARTY_Y);
    let round_1_pre_vote_s =
        t.make_pre_vote_msg(1, Value::Zero, &round_1_just_0, &TestNet::PARTY_S);

    t.abba
        .receive_message(TestNet::PARTY_Y, round_1_pre_vote_y)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_S, round_1_pre_vote_s)
        .unwrap();

    let sign_bytes = t.abba.pre_vote_bytes_to_sign(1, &Value::Zero).unwrap();
    let sig = t.sec_key_set.secret_key().sign(sign_bytes);
    let main_vote_just = MainVoteJustification::NoAbstain(sig);
    let round_1_main_vote_x =
        t.make_main_vote_msg(1, MainVoteValue::zero(), &main_vote_just, &TestNet::PARTY_X);
    let round_1_main_vote_y =
        t.make_main_vote_msg(1, MainVoteValue::zero(), &main_vote_just, &TestNet::PARTY_Y);
    let round_1_main_vote_s =
        t.make_main_vote_msg(1, MainVoteValue::zero(), &main_vote_just, &TestNet::PARTY_S);
    assert!(t.is_broadcasted(&round_1_main_vote_x));

    t.abba
        .receive_message(TestNet::PARTY_Y, round_1_main_vote_y)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_S, round_1_main_vote_s)
        .unwrap();

    assert_eq!(t.abba.decided_value.unwrap().value, Value::Zero);
}

// Byzantine node is offline and slow node doesn't receive the proposal on time.
//
// PARTY_X:
// PreVoteAction  { round: 1, value: One,        justification: WithValidity(...) })
// MainVoteAction { round: 1, value: Abstain,    justification: Abstain(...) })
// PreVoteAction  { round: 2, value: One,        justification: WithValidity(...) })
// MainVoteAction { round: 2, value: Value(One), justification: NoAbstain(...) })
//
// PARTY_Y:
// PreVoteAction  { round: 1, value: One,        justification: WithValidity(...),  })
// MainVoteAction { round: 1, value: Abstain,    justification: Abstain(),  })
// PreVoteAction  { round: 2, value: One,        justification: WithValidity(...),  })
// MainVoteAction { round: 2, value: Value(One), justification: NoAbstain(...)),  })
//
// PARTY_B:
// Offline
//
// PARTY_S:
// PreVoteAction  { round: 1, value: Zero,       justification: FirstRoundZero,  })
// MainVoteAction { round: 1, value: Abstain,    justification: Abstain(FirstRoundZero, WithValidity(...)),  })
// PreVoteAction  { round: 2, value: One,        justification: WithValidity(...),  })
// MainVoteAction { round: 2, value: Value(One), justification: NoAbstain(...)),  })

#[test]
fn test_two_rounds() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    t.abba
        .pre_vote_one(t.proposal_digest, t.proposal_sig.clone())
        .unwrap();

    let round_1_just_0 = PreVoteJustification::FirstRoundZero;
    let weak_validity_just =
        PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());

    let round_1_pre_vote_y =
        t.make_pre_vote_msg(1, Value::One, &weak_validity_just, &TestNet::PARTY_Y);
    let round_1_pre_vote_s =
        t.make_pre_vote_msg(1, Value::Zero, &round_1_just_0, &TestNet::PARTY_S);

    t.abba
        .receive_message(TestNet::PARTY_Y, round_1_pre_vote_y)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_S, round_1_pre_vote_s)
        .unwrap();

    let round_1_main_vote_just = MainVoteJustification::Abstain(
        Box::new(round_1_just_0),
        Box::new(weak_validity_just.clone()),
    );
    let round_1_main_vote_x = t.make_main_vote_msg(
        1,
        MainVoteValue::Abstain,
        &round_1_main_vote_just,
        &TestNet::PARTY_X,
    );
    let round_1_main_vote_y = t.make_main_vote_msg(
        1,
        MainVoteValue::Abstain,
        &round_1_main_vote_just,
        &TestNet::PARTY_Y,
    );
    let round_1_main_vote_s = t.make_main_vote_msg(
        1,
        MainVoteValue::Abstain,
        &round_1_main_vote_just,
        &TestNet::PARTY_S,
    );
    assert!(t.is_broadcasted(&round_1_main_vote_x));

    t.abba
        .receive_message(TestNet::PARTY_Y, round_1_main_vote_y)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_S, round_1_main_vote_s)
        .unwrap();

    // Round 2
    let round_2_pre_vote_x =
        t.make_pre_vote_msg(2, Value::One, &weak_validity_just, &TestNet::PARTY_X);
    let round_2_pre_vote_y =
        t.make_pre_vote_msg(2, Value::One, &weak_validity_just, &TestNet::PARTY_Y);
    let round_2_pre_vote_s =
        t.make_pre_vote_msg(2, Value::One, &weak_validity_just, &TestNet::PARTY_S);

    assert!(t.is_broadcasted(&round_2_pre_vote_x));

    t.abba
        .receive_message(TestNet::PARTY_Y, round_2_pre_vote_y)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_S, round_2_pre_vote_s)
        .unwrap();

    let sign_bytes = t.abba.pre_vote_bytes_to_sign(2, &Value::One).unwrap();
    let sig = t.sec_key_set.secret_key().sign(sign_bytes);
    let round_2_main_vote_just = MainVoteJustification::NoAbstain(sig);
    let round_2_main_vote_x = t.make_main_vote_msg(
        2,
        MainVoteValue::one(),
        &round_2_main_vote_just,
        &TestNet::PARTY_X,
    );
    let round_2_main_vote_y = t.make_main_vote_msg(
        2,
        MainVoteValue::one(),
        &round_2_main_vote_just,
        &TestNet::PARTY_Y,
    );
    let round_2_main_vote_s = t.make_main_vote_msg(
        2,
        MainVoteValue::one(),
        &round_2_main_vote_just,
        &TestNet::PARTY_S,
    );
    assert!(t.is_broadcasted(&round_2_main_vote_x));

    t.abba
        .receive_message(TestNet::PARTY_Y, round_2_main_vote_y)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_S, round_2_main_vote_s)
        .unwrap();

    assert_eq!(t.abba.decided_value.unwrap().value, Value::One);
}

// The slow party is the proposer but other parties receives the proposal after casting their pre-vote to zero.
// The Byzantine node, create a main-vote and send it to the other parties.
//
//
// PARTY_X:
// PreVoteAction  { round: 1, value: Zero,       justification: FirstRoundZero }) }
// MainVoteAction { round: 1, value: Abstain,    justification: Abstain(...) }) }
//
// PARTY_Y:
// PreVoteAction  { round: 1, value: Zero,       justification: FirstRoundZero }) }
// MainVoteAction { round: 1, value: Abstain,    justification: Abstain(...) }) }
// PreVoteAction  { round: 2, value: One,        justification: WithValidity(...)) }) }
//
// PARTY_B:
// PreVoteAction  { round: 1, value: Zero,       justification: FirstRoundZero }) }
// MainVoteAction { round: 1, value: Value(Zero),justification: NoAbstain(...) }) }
// PreVoteAction  { round: 2, value: Zero,       justification: Hard(...) }) }
//
// PARTY_S:
// PreVoteAction  { round: 1, value: One,        justification: WithValidity(...) }) }
// MainVoteAction { round: 1, value: Abstain,    justification: Abstain(...) }) }
// PreVoteAction  { round: 2, value: One,        justification: WithValidity(...)) }) }
// MainVoteAction { round: 2, value: Abstain,    justification: Abstain(...) }) }
// PreVoteAction  { round: 3, value: One,        justification: WithValidity(...),  })
// MainVoteAction { round: 3, value: Value(One), justification: NoAbstain(...)),  })
//
#[test]
fn test_three_rounds() {
    let i = TestNet::PARTY_S;
    let j = TestNet::PARTY_S;
    let mut t = TestNet::new(i, j);

    t.abba
        .pre_vote_one(t.proposal_digest, t.proposal_sig.clone())
        .unwrap();

    let round_1_just_0 = PreVoteJustification::FirstRoundZero;
    let weak_validity_just =
        PreVoteJustification::WithValidity(t.proposal_digest, t.proposal_sig.clone());

    let round_1_pre_vote_x =
        t.make_pre_vote_msg(1, Value::Zero, &round_1_just_0, &TestNet::PARTY_X);
    let round_1_pre_vote_y =
        t.make_pre_vote_msg(1, Value::Zero, &round_1_just_0, &TestNet::PARTY_Y);

    t.abba
        .receive_message(TestNet::PARTY_X, round_1_pre_vote_x)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_Y, round_1_pre_vote_y)
        .unwrap();

    let round_1_main_vote_s = t.make_main_vote_msg(
        1,
        MainVoteValue::Abstain,
        &MainVoteJustification::Abstain(
            Box::new(round_1_just_0.clone()),
            Box::new(weak_validity_just.clone()),
        ),
        &TestNet::PARTY_S,
    );

    let round_1_main_vote_x = t.make_main_vote_msg(
        1,
        MainVoteValue::Abstain,
        &MainVoteJustification::Abstain(
            Box::new(round_1_just_0),
            Box::new(weak_validity_just.clone()),
        ),
        &TestNet::PARTY_X,
    );

    let sign_bytes = t.abba.pre_vote_bytes_to_sign(1, &Value::Zero).unwrap();
    let pre_vote_0_sig = t.sec_key_set.secret_key().sign(sign_bytes);
    let round_1_main_vote_b = t.make_main_vote_msg(
        1,
        MainVoteValue::zero(),
        &MainVoteJustification::NoAbstain(pre_vote_0_sig.clone()),
        &TestNet::PARTY_B,
    );
    assert!(t.is_broadcasted(&round_1_main_vote_s));

    t.abba
        .receive_message(TestNet::PARTY_X, round_1_main_vote_x)
        .unwrap();

    t.abba
        .receive_message(TestNet::PARTY_B, round_1_main_vote_b)
        .unwrap();

    // Round 2
    let round_2_pre_vote_s =
        t.make_pre_vote_msg(2, Value::One, &weak_validity_just, &TestNet::PARTY_S);
    assert!(t.is_broadcasted(&round_2_pre_vote_s));

    let round_2_pre_vote_x =
        t.make_pre_vote_msg(2, Value::One, &weak_validity_just, &TestNet::PARTY_X);
    let round_2_pre_vote_b = t.make_pre_vote_msg(
        2,
        Value::Zero,
        &PreVoteJustification::Hard(pre_vote_0_sig.clone()),
        &TestNet::PARTY_B,
    );

    t.abba
        .receive_message(TestNet::PARTY_X, round_2_pre_vote_x)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_B, round_2_pre_vote_b)
        .unwrap();

    let sign_bytes = t.abba.pre_vote_bytes_to_sign(2, &Value::One).unwrap();
    let pre_vote_1_round_2_sig = t.sec_key_set.secret_key().sign(sign_bytes);
    let round_2_main_vote_s = t.make_main_vote_msg(
        2,
        MainVoteValue::Abstain,
        &MainVoteJustification::Abstain(
            Box::new(PreVoteJustification::Hard(pre_vote_0_sig)),
            Box::new(weak_validity_just.clone()),
        ),
        &TestNet::PARTY_S,
    );
    assert!(t.is_broadcasted(&round_2_main_vote_s));

    let round_2_main_vote_x = t.make_main_vote_msg(
        2,
        MainVoteValue::one(),
        &MainVoteJustification::NoAbstain(pre_vote_1_round_2_sig.clone()),
        &TestNet::PARTY_X,
    );
    let round_2_main_vote_y = t.make_main_vote_msg(
        2,
        MainVoteValue::one(),
        &MainVoteJustification::NoAbstain(pre_vote_1_round_2_sig),
        &TestNet::PARTY_Y,
    );

    t.abba
        .receive_message(TestNet::PARTY_X, round_2_main_vote_x)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_Y, round_2_main_vote_y)
        .unwrap();

    // Round 3
    let round_3_pre_vote_s =
        t.make_pre_vote_msg(3, Value::One, &weak_validity_just, &TestNet::PARTY_S);
    assert!(t.is_broadcasted(&round_3_pre_vote_s));

    let round_3_pre_vote_x =
        t.make_pre_vote_msg(3, Value::One, &weak_validity_just, &TestNet::PARTY_X);

    let round_3_pre_vote_y =
        t.make_pre_vote_msg(3, Value::One, &weak_validity_just, &TestNet::PARTY_Y);

    t.abba
        .receive_message(TestNet::PARTY_X, round_3_pre_vote_x)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_Y, round_3_pre_vote_y)
        .unwrap();

    let sign_bytes = t.abba.pre_vote_bytes_to_sign(3, &Value::One).unwrap();
    let pre_vote_1_round_3_sig = t.sec_key_set.secret_key().sign(sign_bytes);

    let round_2_main_vote_s = t.make_main_vote_msg(
        3,
        MainVoteValue::one(),
        &MainVoteJustification::NoAbstain(pre_vote_1_round_3_sig.clone()),
        &TestNet::PARTY_S,
    );
    assert!(t.is_broadcasted(&round_2_main_vote_s));

    let round_3_main_vote_x = t.make_main_vote_msg(
        3,
        MainVoteValue::one(),
        &MainVoteJustification::NoAbstain(pre_vote_1_round_3_sig.clone()),
        &TestNet::PARTY_X,
    );
    let round_3_main_vote_y = t.make_main_vote_msg(
        3,
        MainVoteValue::one(),
        &MainVoteJustification::NoAbstain(pre_vote_1_round_3_sig),
        &TestNet::PARTY_Y,
    );

    t.abba
        .receive_message(TestNet::PARTY_X, round_3_main_vote_x)
        .unwrap();
    t.abba
        .receive_message(TestNet::PARTY_Y, round_3_main_vote_y)
        .unwrap();

    assert_eq!(t.abba.decided_value.unwrap().value, Value::One);
}

struct Net {
    tag: Tag,
    secret_key_set: SecretKeySet,
    nodes: BTreeMap<NodeId, Abba>,
    queue: BTreeMap<NodeId, Vec<Bundle>>,
}

impl Net {
    fn new(n: usize, proposer: NodeId) -> Self {
        // we can tolerate < n/3 faults
        let faults = n.saturating_sub(1) / 3;

        // we want to require n - f signature shares but
        // blsttc wants a threshold value where you require t + 1 shares
        // So we just subtract 1 from n - f.
        let threshold = (n - faults).saturating_sub(1);
        let secret_key_set = blsttc::SecretKeySet::random(threshold, &mut rand::thread_rng());
        let public_key_set = secret_key_set.public_keys();
        let tag = Tag::new(Domain::new("test-domain", 0), proposer);

        let nodes = BTreeMap::from_iter((1..=n).into_iter().map(|node_id| {
            let key_share = secret_key_set.secret_key_share(node_id);
            let broadcaster = Rc::new(RefCell::new(Broadcaster::new(node_id)));
            let vcbc = Abba::new(
                tag.clone(),
                node_id,
                public_key_set.clone(),
                key_share,
                broadcaster,
            );
            (node_id, vcbc)
        }));

        Net {
            tag,
            secret_key_set,
            nodes,
            queue: Default::default(),
        }
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Abba {
        self.nodes.get_mut(&id).unwrap()
    }

    fn enqueue_bundles_from(&mut self, id: NodeId) {
        let (send_bundles, bcast_bundles) = {
            let mut broadcaster = self.node_mut(id).broadcaster.borrow_mut();
            let (bcast_bundles, send_bundles) = broadcaster.take_bundles();
            (send_bundles, bcast_bundles)
        };

        for (recipient, bundle) in send_bundles {
            let bundle: Bundle =
                bincode::deserialize(&bundle).expect("Failed to deserialize bundle");
            self.queue.entry(recipient).or_default().push(bundle);
        }

        for bundle in bcast_bundles {
            for recipient in self.nodes.keys() {
                let bundle: Bundle =
                    bincode::deserialize(&bundle).expect("Failed to deserialize bundle");
                self.queue.entry(*recipient).or_default().push(bundle);
            }
        }
    }

    fn drain_queue(&mut self) {
        while !self.queue.is_empty() {
            for (recipient, queue) in std::mem::take(&mut self.queue) {
                let recipient_node = self.node_mut(recipient);

                for bundle in queue {
                    let msg: Message = bincode::deserialize(&bundle.payload)
                        .expect("Failed to deserialize message");

                    println!("Handling message: {msg:?}");
                    recipient_node
                        .receive_message(bundle.initiator, msg)
                        .expect("Failed to receive msg");
                }

                self.enqueue_bundles_from(recipient);
            }
        }
    }

    #[allow(unused)]
    fn deliver(&mut self, recipient: NodeId, index: usize) {
        if let Some(msgs) = self.queue.get_mut(&recipient) {
            if msgs.is_empty() {
                return;
            }
            let index = index % msgs.len();

            let bundle = msgs.swap_remove(index);
            let msg: Message =
                bincode::deserialize(&bundle.payload).expect("Failed to deserialize message");

            let recipient_node = self.node_mut(recipient);
            recipient_node
                .receive_message(bundle.initiator, msg)
                .expect("Failed to receive message");
            self.enqueue_bundles_from(recipient);
        }
    }
}

#[test]
fn test_net_happy_path() {
    let proposer = 1;
    let mut net = Net::new(4, proposer);

    let proposal_digest = Hash32::calculate("test-data".as_bytes());
    let sign_bytes = crate::mvba::vcbc::c_ready_bytes_to_sign(&net.tag, &proposal_digest).unwrap();
    let proposal_sig = net.secret_key_set.secret_key().sign(sign_bytes);

    // All nodes pre-vote one

    for id in Vec::from_iter(net.nodes.keys().copied()) {
        net.node_mut(id)
            .pre_vote_one(proposal_digest, proposal_sig.clone())
            .expect("Failed to pre-vote");

        net.enqueue_bundles_from(id);
    }

    net.drain_queue();

    for (id, node) in net.nodes {
        println!("Checking {id}");
        assert_eq!(node.decided_value.unwrap().value, Value::One);
    }
}
