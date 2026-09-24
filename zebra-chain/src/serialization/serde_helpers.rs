use group::ff::PrimeField;
use halo2::pasta::pallas;

#[derive(Deserialize, Serialize)]
#[serde(remote = "jubjub::Fq")]
pub struct Fq {
    #[serde(getter = "jubjub::Fq::to_bytes")]
    bytes: [u8; 32],
}

impl From<Fq> for jubjub::Fq {
    fn from(local: Fq) -> Self {
        jubjub::Fq::from_bytes(&local.bytes).unwrap()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(remote = "pallas::Base")]
pub struct Base {
    #[serde(getter = "pallas::Base::to_repr")]
    bytes: [u8; 32],
}

impl From<Base> for pallas::Base {
    fn from(local: Base) -> Self {
        pallas::Base::from_repr(local.bytes).unwrap()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(remote = "sapling_crypto::Node")]
pub struct Node {
    #[serde(getter = "Node::as_serializable_bytes")]
    bytes: [u8; 32],
}

impl From<Node> for sapling_crypto::Node {
    fn from(local: Node) -> Self {
        sapling_crypto::Node::from_bytes(local.bytes).unwrap()
    }
}

impl Node {
    fn as_serializable_bytes(remote: &sapling_crypto::Node) -> [u8; 32] {
        remote.to_bytes()
    }
}
