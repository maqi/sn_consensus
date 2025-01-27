use super::message::{Action, Message};
use super::Error;
use super::{NodeId, Vcbc};
use crate::mvba::broadcaster::Broadcaster;
use crate::mvba::bundle::Bundle;
use crate::mvba::hash::Hash32;
use crate::mvba::tag::{Domain, Tag};
use crate::mvba::vcbc::c_ready_bytes_to_sign;
use crate::mvba::Proposal;
use blsttc::{SecretKeySet, Signature, SignatureShare};
use quickcheck_macros::quickcheck;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

fn valid_proposal(_: NodeId, _: &Proposal) -> bool {
    true
}

fn invalid_proposal(_: NodeId, _: &Proposal) -> bool {
    false
}

struct Net {
    domain: Domain,
    secret_key_set: SecretKeySet,
    nodes: BTreeMap<NodeId, Vcbc>,
    queue: BTreeMap<NodeId, Vec<Bundle>>,
}

// TODO: merging Net and TestNet
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
        let domain = Domain::new("testing-vcbc", 0);

        let nodes = BTreeMap::from_iter((1..=n).into_iter().map(|self_id| {
            let key_share = secret_key_set.secret_key_share(self_id);
            let broadcaster = Rc::new(RefCell::new(Broadcaster::new(self_id)));
            let tag = Tag::new(domain.clone(), proposer);
            let vcbc = Vcbc::new(
                tag,
                self_id,
                public_key_set.clone(),
                key_share,
                valid_proposal,
                broadcaster,
            );
            (self_id, vcbc)
        }));

        Net {
            domain,
            secret_key_set,
            nodes,
            queue: Default::default(),
        }
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Vcbc {
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

                    recipient_node
                        .receive_message(bundle.initiator, msg)
                        .expect("Failed to receive msg");
                }

                self.enqueue_bundles_from(recipient);
            }
        }
    }

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
fn test_vcbc_happy_path() {
    let proposer = 1;
    let mut net = Net::new(7, proposer);

    let tag = Tag::new(net.domain.clone(), proposer);

    // Node 1 (the proposer) will initiate VCBC by broadcasting a value

    net.node_mut(proposer)
        .c_broadcast("HAPPY-PATH-VALUE".as_bytes().to_vec())
        .expect("Failed to c-broadcast");

    net.enqueue_bundles_from(proposer);

    // Now we roll-out the simulation to completion.

    net.drain_queue();

    // And check that all nodes have delivered the expected value and signature

    let expected_bytes_to_sign: Vec<u8> =
        c_ready_bytes_to_sign(&tag, &Hash32::calculate("HAPPY-PATH-VALUE"))
            .expect("Failed to serialize");

    let expected_sig = net.secret_key_set.secret_key().sign(expected_bytes_to_sign);

    for (_, node) in net.nodes {
        assert_eq!(
            node.read_delivered(),
            Some(("HAPPY-PATH-VALUE".as_bytes().to_vec(), expected_sig.clone()))
        )
    }
}

#[quickcheck]
fn prop_vcbc_terminates_under_randomized_msg_delivery(
    n: usize,
    proposer: usize,
    proposal: Vec<u8>,
    msg_order: Vec<(NodeId, usize)>,
) {
    let n = n % 10 + 1; // Large n is wasteful, and n must be > 0
    let proposer = proposer % n + 1; // NodeId's start at 1
    let mut net = Net::new(n, proposer);

    // First the proposer will initiate VCBC by broadcasting the proposal:
    let proposer_node = net.node_mut(proposer);
    proposer_node
        .c_broadcast(proposal.clone())
        .expect("Failed to c-broadcast");

    net.enqueue_bundles_from(proposer);

    // Next we deliver the messages in the order chosen by quickcheck
    for (recipient, msg_index) in msg_order {
        net.deliver(recipient, msg_index);
    }

    // Then we roll-out the simulation to completion.
    net.drain_queue();

    // And finally, check that all nodes have delivered the expected value and signature

    let tag = Tag::new(net.domain.clone(), proposer);
    let expected_bytes_to_sign: Vec<u8> =
        c_ready_bytes_to_sign(&tag, &Hash32::calculate(&proposal)).expect("Failed to serialize");

    let expected_sig = net.secret_key_set.secret_key().sign(expected_bytes_to_sign);

    for (_, node) in net.nodes {
        assert_eq!(
            node.read_delivered(),
            Some((proposal.clone(), expected_sig.clone()))
        )
    }
}

#[test]
fn test_ignore_messages_with_invalid_tag() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    let mut final_msg = t.make_final_msg(&t.d());
    final_msg.tag.domain = Domain::new("another-domain", 0);

    let result = t.vcbc.receive_message(TestNet::PARTY_B, final_msg);
    match result {
        Err(Error::InvalidMessage(msg)) => assert_eq!(
            msg,
            "invalid tag. expected test-domain[0].0, got another-domain[0].0"
        ),
        res => panic!("Unexpected result: {res:?}"),
    }
}

// --------------------------------------
// Testing one peers in faulty situations

use rand::{thread_rng, Rng};

struct TestNet {
    sec_key_set: SecretKeySet,
    vcbc: Vcbc,
    m: Vec<u8>,
    broadcaster: Rc<RefCell<Broadcaster>>,
}

impl TestNet {
    const PARTY_X: NodeId = 0;
    const PARTY_Y: NodeId = 1;
    const PARTY_B: NodeId = 2;
    const PARTY_S: NodeId = 3;

    // There are 4 parties: X, Y, B, S (B is Byzantine and S is Slow)
    // The VCBC test instance creates for party `i` with `ID` sets to `test-id`
    // and `s` sets to `0`.
    pub fn new(i: NodeId, j: NodeId) -> Self {
        let mut rng = thread_rng();
        let sec_key_set = SecretKeySet::random(2, &mut rng);
        let sec_key_share = sec_key_set.secret_key_share(i);
        let broadcaster = Rc::new(RefCell::new(Broadcaster::new(i)));
        let tag = Tag::new(Domain::new("test-domain", 0), j);
        let vcbc = Vcbc::new(
            tag,
            i,
            sec_key_set.public_keys(),
            sec_key_share,
            valid_proposal,
            broadcaster.clone(),
        );

        // Creating a random proposal
        let m = (0..100).map(|_| rng.gen_range(0..64)).collect();

        Self {
            sec_key_set,
            vcbc,
            m,
            broadcaster,
        }
    }

    pub fn make_send_msg(&self, m: &[u8]) -> Message {
        Message {
            tag: self.vcbc.tag.clone(),
            action: Action::Send(m.to_vec()),
        }
    }

    pub fn make_ready_msg(&self, d: &Hash32, peer_id: &NodeId) -> Message {
        let sig_share = self.sig_share(d, peer_id);
        Message {
            tag: self.vcbc.tag.clone(),
            action: Action::Ready(*d, sig_share),
        }
    }

    pub fn make_final_msg(&self, d: &Hash32) -> Message {
        Message {
            tag: self.vcbc.tag.clone(),
            action: Action::Final(*d, self.u()),
        }
    }

    pub fn is_broadcasted(&self, msg: &Message) -> bool {
        self.broadcaster
            .borrow()
            .has_gossip_message(&bincode::serialize(msg).unwrap())
    }

    pub fn is_send_to(&self, to: &NodeId, msg: &Message) -> bool {
        self.broadcaster
            .borrow()
            .has_direct_message(to, &bincode::serialize(msg).unwrap())
    }

    // m is same as proposal
    pub fn m(&self) -> Vec<u8> {
        self.m.clone()
    }

    // d is same as proposal's digest
    pub fn d(&self) -> Hash32 {
        Hash32::calculate(&self.m)
    }

    // u is same as final signature
    pub fn u(&self) -> Signature {
        let sign_bytes = c_ready_bytes_to_sign(&self.vcbc.tag, &self.d()).unwrap();
        self.sec_key_set.secret_key().sign(sign_bytes)
    }

    fn sig_share(&self, digest: &Hash32, id: &NodeId) -> SignatureShare {
        let sign_bytes = c_ready_bytes_to_sign(&self.vcbc.tag, digest).unwrap();
        let sec_key_share = self.sec_key_set.secret_key_share(id);

        sec_key_share.sign(sign_bytes)
    }
}

#[test]
fn test_invalid_message() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_B;
    let mut t = TestNet::new(i, j);
    t.vcbc.message_validity = invalid_proposal;

    let msg = t.make_send_msg(&t.m);

    let result = t.vcbc.receive_message(TestNet::PARTY_B, msg);
    assert!(matches!(result, Err(Error::InvalidMessage(msg))
    if msg == *"invalid proposal"));
}

#[test]
fn test_should_c_send() {
    let i = TestNet::PARTY_S;
    let j = TestNet::PARTY_S; // i and j are same
    let mut t = TestNet::new(i, j);

    t.vcbc.c_broadcast(t.m.clone()).unwrap();

    let send_msg = t.make_send_msg(&t.m());
    assert!(t.is_broadcasted(&send_msg));
}

#[test]
fn test_should_c_ready() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_S;
    let mut t = TestNet::new(i, j);

    let send_msg = t.make_send_msg(&t.m());
    t.vcbc.receive_message(j, send_msg).unwrap();

    let ready_msg_x = t.make_ready_msg(&t.d(), &i);
    assert!(t.is_send_to(&j, &ready_msg_x));
}

#[test]
fn test_normal_case_operation() {
    let i = TestNet::PARTY_S;
    let j = TestNet::PARTY_S; // i and j are same
    let mut t = TestNet::new(i, j);

    t.vcbc.c_broadcast(t.m.clone()).unwrap();

    let ready_msg_x = t.make_ready_msg(&t.d(), &TestNet::PARTY_X);
    let ready_msg_y = t.make_ready_msg(&t.d(), &TestNet::PARTY_Y);

    t.vcbc
        .receive_message(TestNet::PARTY_X, ready_msg_x)
        .unwrap();
    t.vcbc
        .receive_message(TestNet::PARTY_Y, ready_msg_y)
        .unwrap();

    assert!(t.vcbc.read_delivered().is_some());
}

#[test]
fn test_final_message_first() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_S;
    let mut t = TestNet::new(i, j);

    let send_msg = t.make_send_msg(&t.m());
    let final_msg = t.make_final_msg(&t.d());

    t.vcbc.receive_message(TestNet::PARTY_S, final_msg).unwrap();
    t.vcbc.receive_message(TestNet::PARTY_S, send_msg).unwrap();

    assert!(t.vcbc.read_delivered().is_some());
}

#[test]
fn test_request_for_proposal() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_S;
    let mut t = TestNet::new(i, j);

    let final_msg = t.make_final_msg(&t.d());
    let request_msg = Message {
        tag: t.vcbc.tag.clone(),
        action: Action::Request,
    };

    t.vcbc.receive_message(TestNet::PARTY_S, final_msg).unwrap();
    assert!(t.is_send_to(&TestNet::PARTY_S, &request_msg));
}

#[test]
fn test_invalid_digest() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    t.vcbc.c_broadcast(t.m.clone()).unwrap();

    let invalid_digest = Hash32::calculate("invalid-data");
    let ready_msg_x = t.make_ready_msg(&invalid_digest, &i);
    assert!(t
        .vcbc
        .receive_message(TestNet::PARTY_B, ready_msg_x)
        .is_err());
}

#[test]
fn test_invalid_sig_share() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    t.vcbc.c_broadcast(t.m.clone()).unwrap();

    let sig_share = t
        .sec_key_set
        .secret_key_share(TestNet::PARTY_B)
        .sign("invalid_message".as_bytes());
    let ready_msg_x = Message {
        tag: t.vcbc.tag.clone(),
        action: Action::Ready(t.d(), sig_share),
    };

    t.vcbc
        .receive_message(TestNet::PARTY_B, ready_msg_x)
        .unwrap();

    assert!(!t.vcbc.wd.contains_key(&TestNet::PARTY_B));
}

#[test]
fn test_c_request() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_X;
    let mut t = TestNet::new(i, j);

    let sig = t.sec_key_set.secret_key().sign(t.m());

    t.vcbc.m_bar = Some(t.m());
    t.vcbc.u_bar = Some(sig.clone());

    let request_msg = Message {
        tag: t.vcbc.tag.clone(),
        action: Action::Request,
    };
    let answer_msg = Message {
        tag: t.vcbc.tag.clone(),
        action: Action::Answer(t.m(), sig),
    };

    t.vcbc
        .receive_message(TestNet::PARTY_S, request_msg)
        .unwrap();

    assert!(t.is_send_to(&TestNet::PARTY_S, &answer_msg));
}

#[test]
fn test_c_answer() {
    let i = TestNet::PARTY_X;
    let j = TestNet::PARTY_S;
    let mut t = TestNet::new(i, j);

    let sig = t.u();
    let answer_msg = Message {
        tag: t.vcbc.tag.clone(),
        action: Action::Answer(t.m(), sig),
    };

    t.vcbc
        .receive_message(TestNet::PARTY_S, answer_msg)
        .unwrap();

    assert!(t.vcbc.read_delivered().is_some());
}
