use super::*;
use crate::mvba::crypto::public::random_pub_key;
use minicbor::to_vec;
use rand::{random, Rng};

struct TestData {
    party_x: PubKey,
    party_y: PubKey,
    party_b: PubKey,
    party_s: PubKey,
    vcbc: Vcbc,
    broadcaster: Rc<RefCell<Broadcaster>>,
    proposal: Proposal,
}

fn valid_proposal(_: &Proposal) -> bool {
    true
}

fn invalid_proposal(_: &Proposal) -> bool {
    false
}

impl TestData {
    // There are 4 parties: X, Y, B, S (B is Byzantine and S is Slow)
    // The VCBC test instance is created for party X.
    pub fn new(proposer: &str) -> Self {
        let party_x = random_pub_key();
        let party_y = random_pub_key();
        let party_b = random_pub_key();
        let party_s = random_pub_key();
        let parties = vec![
            party_x.clone(),
            party_y.clone(),
            party_b.clone(),
            party_s.clone(),
        ];
        let proposer = match proposer {
            "x" => party_x.clone(),
            "y" => party_y.clone(),
            "b" => party_b.clone(),
            "s" => party_s.clone(),
            _ => panic!("invalid proposer"),
        };
        let broadcaster = Rc::new(RefCell::new(Broadcaster::new(random(), &party_x)));
        let vcbc = Vcbc::new(
            proposer.clone(),
            parties,
            1,
            broadcaster.clone(),
            valid_proposal,
        );

        // Creating a random proposal
        let mut rng = rand::thread_rng();
        let proposal = Proposal {
            proposer,
            value: (0..100).map(|_| rng.gen_range(0..64)).collect(),
            proof: (0..100).map(|_| rng.gen_range(0..64)).collect(),
        };

        Self {
            party_x,
            party_y,
            party_b,
            party_s,
            vcbc,
            broadcaster,
            proposal,
        }
    }

    pub fn propose_msg(&self) -> Vec<u8> {
        self.make_msg(message::MSG_TAG_PROPOSE)
    }

    pub fn echo_msg(&self) -> Vec<u8> {
        self.make_msg(message::MSG_TAG_ECHO)
    }

    pub fn make_msg(&self, tag: &str) -> Vec<u8> {
        to_vec(Message {
            proposal: self.proposal.clone(),
            tag: tag.to_string(),
        })
        .unwrap()
    }
    pub fn is_proposed(&self) -> bool {
        self.broadcaster.borrow().has_message(&self.propose_msg())
    }
    pub fn is_echoed(&self) -> bool {
        self.broadcaster.borrow().has_message(&self.echo_msg())
    }
}

#[test]
fn test_propose() {
    let mut t = TestData::new("x");

    t.vcbc.propose(&t.proposal).unwrap();

    assert!(t.is_proposed());
    assert!(t.is_echoed());
    assert!(t.vcbc.ctx.echos.contains(&t.party_x));
}

#[test]
fn test_normal_case() {
    let mut t = TestData::new("x");

    assert!(!t.vcbc.is_delivered());
    assert_eq!(t.vcbc.ctx.proposal, None);
    assert!(t.vcbc.ctx.echos.is_empty());

    t.vcbc.propose(&t.proposal).unwrap();
    t.vcbc.process_message(&t.party_y, &t.echo_msg()).unwrap();
    t.vcbc.process_message(&t.party_s, &t.echo_msg()).unwrap();

    assert!(t.vcbc.is_delivered());
    assert_eq!(t.vcbc.ctx.proposal, Some(t.proposal));
    assert!(&t.vcbc.ctx.echos.contains(&t.party_x));
    assert!(&t.vcbc.ctx.echos.contains(&t.party_y));
    assert!(&t.vcbc.ctx.echos.contains(&t.party_s));
}

#[test]
fn test_delayed_propose_message() {
    let mut t = TestData::new("s");

    t.vcbc.process_message(&t.party_y, &t.echo_msg()).unwrap();
    t.vcbc.process_message(&t.party_s, &t.echo_msg()).unwrap();

    assert!(t.vcbc.is_delivered());

    // Receiving propose message now
    t.broadcaster.borrow_mut().clear();
    t.vcbc
        .process_message(&t.party_s, &t.propose_msg())
        .unwrap();

    assert!(!t.is_echoed());
}

#[test]
fn test_invalid_proposal() {
    let mut t = TestData::new("b");
    t.vcbc.ctx.proposal_checker = invalid_proposal;

    assert_eq!(
        t.vcbc.process_message(&t.party_b, &t.propose_msg()).err(),
        Some(Error::InvalidProposal(t.proposal)),
    );
}

#[test]
fn test_duplicated_proposal() {
    let mut t = TestData::new("b");

    // Party_x receives a proposal from party_b
    t.vcbc
        .process_message(&t.party_b, &t.propose_msg())
        .unwrap();

    // Party_x receives an echo message from from party_s
    // that echoes different proposal
    let mut rng = rand::thread_rng();
    let duplicated_proposal = Proposal {
        proposer: t.party_b.clone(),
        value: (0..100).map(|_| rng.gen_range(0..64)).collect(),
        proof: (0..100).map(|_| rng.gen_range(0..64)).collect(),
    };
    let data = to_vec(Message {
        proposal: duplicated_proposal.clone(),
        tag: message::MSG_TAG_ECHO.to_string(),
    })
    .unwrap();

    assert_eq!(
        t.vcbc.process_message(&t.party_b, &data).err(),
        Some(Error::DuplicatedProposal(duplicated_proposal)),
    );
}
