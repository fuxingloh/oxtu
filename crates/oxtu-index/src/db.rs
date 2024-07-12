use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Range;

use rocksdb::{
    ColumnFamilyDescriptor, DBIteratorWithThreadMode, Direction, IteratorMode, Options,
    ReadOptions, SliceTransform, TransactionDB, TransactionDBOptions, WriteBatchWithTransaction,
};
use serde::{Deserialize, Serialize};

use crate::types::{U128Decimal, U256};

mod bincode {
    use bincode::{DefaultOptions, Options};

    pub fn serialize<T>(value: &T) -> bincode::Result<Vec<u8>>
    where
        T: ?Sized + serde::Serialize,
    {
        DefaultOptions::new()
            .with_varint_encoding()
            .with_big_endian()
            .serialize(value)
    }

    pub fn deserialize<'a, T>(bytes: &'a [u8]) -> bincode::Result<T>
    where
        T: serde::de::Deserialize<'a>,
    {
        DefaultOptions::new()
            .with_varint_encoding()
            .with_big_endian()
            .deserialize(bytes)
    }
}

trait CFStruct: Sized {
    type Key: Clone + Serialize + for<'de> Deserialize<'de>;
    type KeyRef<'a>: Serialize;
    type Value: Serialize + for<'de> Deserialize<'de>;

    const CF_NAME: &'static str;

    fn new_cf_descriptor() -> ColumnFamilyDescriptor {
        ColumnFamilyDescriptor::new(Self::CF_NAME, Options::default())
    }

    fn key(&self) -> Cow<Self::Key>;

    fn value(&self) -> Self::Value;

    fn assemble(key: Self::Key, value: Self::Value) -> Self;

    fn decode(kv: (&[u8], &[u8])) -> Self {
        let key: Self::Key = bincode::deserialize(kv.0).unwrap();
        let value: Self::Value = bincode::deserialize(kv.1).unwrap();
        Self::assemble(key, value)
    }

    fn encode(&self) -> (Vec<u8>, Vec<u8>) {
        let key = bincode::serialize(&self.key()).unwrap();
        let value = bincode::serialize(&self.value()).unwrap();
        (key, value)
    }

    fn batch_put(
        rocksdb: &TransactionDB,
        batch: &mut WriteBatchWithTransaction<true>,
        data: &Self,
    ) {
        let family = rocksdb.cf_handle(Self::CF_NAME).unwrap();
        let (key, value) = data.encode();
        batch.put_cf(family, &key, &value);
    }

    fn batch_delete(
        rocksdb: &TransactionDB,
        batch: &mut WriteBatchWithTransaction<true>,
        key: Self::KeyRef<'_>,
    ) {
        let family = rocksdb.cf_handle(Self::CF_NAME).unwrap();
        let key = bincode::serialize(&key).unwrap();
        batch.delete_cf(family, &key);
    }

    fn read(rocksdb: &TransactionDB, key: Self::KeyRef<'_>) -> Option<Self> {
        let family = rocksdb.cf_handle(Self::CF_NAME).unwrap();
        let key = bincode::serialize(&key).unwrap();
        rocksdb
            .get_pinned_cf(family, &key)
            .unwrap()
            .map(|value| Self::decode((&key, &value)))
    }

    fn iterator<'a>(
        rocksdb: &'a TransactionDB,
        readopts: ReadOptions,
        mode: IteratorMode,
    ) -> CFIterator<'a, Self> {
        let family = rocksdb.cf_handle(Self::CF_NAME).unwrap();
        let iter = rocksdb.iterator_cf_opt(family, readopts, mode);
        CFIterator::<Self> {
            inner: iter,
            phantom: PhantomData,
        }
    }
}

pub struct CFIterator<'a, D> {
    inner: DBIteratorWithThreadMode<'a, TransactionDB>,
    phantom: PhantomData<D>,
}

impl<'a, D: CFStruct> Iterator for CFIterator<'a, D> {
    type Item = D;

    fn next(&mut self) -> Option<D> {
        self.inner.next().map(|x| {
            let (key, value) = x.unwrap();
            D::decode((&key, &value))
        })
    }
}

pub struct Block {
    pub height: u64,
    pub hash: U256,
    pub prev_hash: U256,
}

impl CFStruct for Block {
    type Key = u64;
    type KeyRef<'a> = &'a u64;
    type Value = (U256, U256);

    const CF_NAME: &'static str = "block";

    fn key(&self) -> Cow<Self::Key> {
        Cow::Borrowed(&self.height)
    }

    fn value(&self) -> Self::Value {
        (self.hash, self.prev_hash)
    }

    fn assemble(height: Self::Key, (hash, prev_hash): Self::Value) -> Self {
        Self {
            height,
            hash,
            prev_hash,
        }
    }
}

struct BlockUndo {
    height: u64,
    vec: Vec<Undo>,
}

#[derive(Serialize, Deserialize, Clone)]
enum Undo {
    UtxoPut(Utxo),
    UtxoDelete(<Utxo as CFStruct>::Key),
    UtxoKeyPut(UtxoKey),
    UtxoKeyDelete(<UtxoKey as CFStruct>::Key),
    ScriptInfoPut(ScriptInfo),
    ScriptInfoDelete(<ScriptInfo as CFStruct>::Key),
}

impl CFStruct for BlockUndo {
    type Key = u64;
    type KeyRef<'a> = &'a u64;
    type Value = Vec<Undo>;

    const CF_NAME: &'static str = "block_undo";

    fn key(&self) -> Cow<Self::Key> {
        Cow::Borrowed(&self.height)
    }

    fn value(&self) -> Self::Value {
        self.vec.clone()
    }

    fn assemble(height: Self::Key, vec: Self::Value) -> Self {
        Self { height, vec }
    }
}

/// Vout represents a transaction output in a transaction.
/// Where `n` is the index of the output in the transaction.
#[derive(Serialize, Deserialize, Copy, Clone, Eq, Hash, PartialEq)]
pub struct Vout {
    pub txid: U256,
    pub n: u32,
}

impl fmt::Display for Vout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Vout {{ txid: {}, n: {} }}", self.txid.to_hex(), self.n)
    }
}

/// Utxo is used to lookup UTXO by (script, txid, vout) to provide the listunspent functionality.
/// Key: (script, txid, vout) -> Value: (height, version, locktime, value)
#[derive(Serialize, Deserialize, Clone)]
pub struct Utxo {
    pub key: UtxoKey,
    pub coinbase: bool,
    pub value: U128Decimal,
}

impl CFStruct for Utxo {
    type Key = UtxoKey;
    type KeyRef<'a> = &'a UtxoKey;
    type Value = (bool, U128Decimal);

    const CF_NAME: &'static str = "utxo";

    fn new_cf_descriptor() -> ColumnFamilyDescriptor {
        let mut options = Options::default();
        options.set_prefix_extractor(SliceTransform::create(
            "ScriptPrefix",
            |key| {
                const SINGLE_BYTE_MAX: u8 = 250;
                const U16_BYTE: u8 = 251;

                match key[0] {
                    byte @ 0..=SINGLE_BYTE_MAX => &key[..(byte + 1) as usize],
                    U16_BYTE => {
                        &key[..(3 + u16::from_be_bytes(key[1..3].try_into().unwrap()) as usize)]
                    }
                    _ => {
                        panic!("Byte type not supported")
                    }
                }
            },
            None,
        ));

        ColumnFamilyDescriptor::new(Self::CF_NAME, options)
    }

    fn key(&self) -> Cow<Self::Key> {
        Cow::Borrowed(&self.key)
    }

    fn value(&self) -> Self::Value {
        (self.coinbase, self.value)
    }

    fn assemble(key: Self::Key, value: Self::Value) -> Self {
        let (coinbase, value) = value;
        Self {
            key,
            coinbase,
            value,
        }
    }
}

/// UtxoKey is used to lookup a UtxoKey with a vout.
/// Key: (txid, n) -> Value: (script, height)
#[derive(Serialize, Deserialize, Clone)]
pub struct UtxoKey {
    pub script: Vec<u8>,
    pub height: u64,
    pub vout: Vout,
}

impl CFStruct for UtxoKey {
    type Key = Vout;
    type KeyRef<'a> = &'a Vout;
    type Value = (u64, Vec<u8>);

    const CF_NAME: &'static str = "utxo_key";

    fn key(&self) -> Cow<Self::Key> {
        Cow::Borrowed(&self.vout)
    }

    fn value(&self) -> Self::Value {
        (self.height, self.script.clone())
    }

    fn assemble(vout: Self::Key, value: Self::Value) -> Self {
        let (height, script) = value;
        Self {
            vout,
            script,
            height,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ScriptInfo {
    pub script: Vec<u8>,
    pub balance: U128Decimal,
    pub total_sent: U128Decimal,
    pub total_received: U128Decimal,
    pub tx_count: u64,
}

impl ScriptInfo {
    fn new(script: &[u8]) -> Self {
        Self {
            script: script.to_vec(),
            balance: U128Decimal::zero(),
            total_sent: U128Decimal::zero(),
            total_received: U128Decimal::zero(),
            tx_count: 0,
        }
    }

    fn add_unspent(&mut self, value: U128Decimal) {
        self.balance += value;
        self.total_received += value;
        self.tx_count += 1;
    }

    fn add_spent(&mut self, value: U128Decimal) {
        self.balance -= value;
        self.total_sent += value;
        self.tx_count += 1;
    }
}

impl CFStruct for ScriptInfo {
    type Key = Vec<u8>;
    type KeyRef<'a> = &'a [u8];
    type Value = (U128Decimal, U128Decimal, U128Decimal, u64);

    const CF_NAME: &'static str = "script_info";

    fn key(&self) -> Cow<Self::Key> {
        Cow::Borrowed(&self.script)
    }

    fn value(&self) -> Self::Value {
        (
            self.balance,
            self.total_sent,
            self.total_received,
            self.tx_count,
        )
    }

    fn assemble(key: Self::Key, value: Self::Value) -> Self {
        let (balance, total_sent, total_received, tx_count) = value;
        Self {
            script: key,
            balance,
            total_sent,
            total_received,
            tx_count,
        }
    }
}

#[must_use]
pub struct Db {
    rocksdb: TransactionDB,
}

impl Db {
    pub fn open(path: &str) -> Self {
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        let tx_options = TransactionDBOptions::default();

        let cfs = vec![
            Block::new_cf_descriptor(),
            BlockUndo::new_cf_descriptor(),
            Utxo::new_cf_descriptor(),
            UtxoKey::new_cf_descriptor(),
            ScriptInfo::new_cf_descriptor(),
        ];

        let rocksdb = TransactionDB::open_cf_descriptors(&options, &tx_options, path, cfs)
            .expect("Failed to open database");

        Self { rocksdb }
    }

    pub fn peek(&self) -> Option<Block> {
        Block::iterator(&self.rocksdb, ReadOptions::default(), IteratorMode::End).next()
    }

    pub fn pop(&self) -> Block {
        let block = self.peek().expect("Failed to pop");

        let mut batch = WriteBatchWithTransaction::default();
        Block::batch_delete(&self.rocksdb, &mut batch, &block.key());

        let block_undo = BlockUndo::read(&self.rocksdb, &block.height).unwrap();
        BlockUndo::batch_delete(&self.rocksdb, &mut batch, &block_undo.key());

        for undo in block_undo.vec.iter() {
            match undo {
                Undo::UtxoPut(utxo) => {
                    Utxo::batch_put(&self.rocksdb, &mut batch, utxo);
                }
                Undo::UtxoDelete(key) => {
                    Utxo::batch_delete(&self.rocksdb, &mut batch, key);
                }
                Undo::UtxoKeyPut(vout_script) => {
                    UtxoKey::batch_put(&self.rocksdb, &mut batch, vout_script);
                }
                Undo::UtxoKeyDelete(key) => {
                    UtxoKey::batch_delete(&self.rocksdb, &mut batch, key);
                }
                Undo::ScriptInfoPut(info) => {
                    ScriptInfo::batch_put(&self.rocksdb, &mut batch, info);
                }
                Undo::ScriptInfoDelete(key) => {
                    ScriptInfo::batch_delete(&self.rocksdb, &mut batch, key);
                }
            }
        }

        block
    }

    pub fn push(&self, rpc_block: crate::rpc::Block) {
        let mut batch = WriteBatchWithTransaction::default();
        let height: u64 = rpc_block.height;

        let mut undos = Vec::<Undo>::new();
        let mut utxos = HashMap::<Vout, Utxo>::new();
        let mut infos = HashMap::<Vec<u8>, ScriptInfo>::new();

        let mut update_info =
            |undos: &mut Vec<Undo>, script: &[u8], f: &dyn Fn(&mut ScriptInfo)| match infos
                .get_mut(script)
            {
                Some(info) => f(info),
                None => {
                    let mut info = match ScriptInfo::read(&self.rocksdb, script) {
                        None => {
                            let info = ScriptInfo::new(script);
                            undos.push(Undo::ScriptInfoDelete(info.key().into_owned()));
                            info
                        }
                        Some(info) => {
                            undos.push(Undo::ScriptInfoPut(info.clone()));
                            info
                        }
                    };
                    f(&mut info);
                    infos.insert(script.to_vec(), info);
                }
            };

        for tx in rpc_block.tx {
            let txid = U256::from_hex(&tx.txid);
            let mut coinbase = false;

            for tx_vin in tx.vin {
                match tx_vin.txid.as_ref() {
                    Some(txid) => {
                        let vout = Vout {
                            txid: U256::from_hex(txid),
                            n: tx_vin.vout.expect("Vout is missing when txid is present"),
                        };
                        match utxos.remove(&vout) {
                            None => {
                                let utxo = self.get_utxo(&vout);
                                update_info(&mut undos, &utxo.key.script, &|info| {
                                    info.add_spent(utxo.value);
                                });
                                undos.push(Undo::UtxoKeyPut(utxo.key().into_owned()));
                                undos.push(Undo::UtxoPut(utxo));
                            }
                            Some(utxo) => {
                                update_info(&mut undos, &utxo.key.script, &|info| {
                                    info.add_spent(utxo.value);
                                });
                            }
                        }
                    }
                    None => {
                        coinbase = true;
                    }
                }
            }

            for tx_vout in tx.vout {
                let utxo = Utxo {
                    key: UtxoKey {
                        script: hex::decode(&tx_vout.script_pub_key.hex).unwrap(),
                        vout: Vout { txid, n: tx_vout.n },
                        height,
                    },
                    coinbase,
                    value: tx_vout.value.into(),
                };

                update_info(&mut undos, &utxo.key.script, &|info| {
                    info.add_unspent(utxo.value);
                });

                utxos.insert(utxo.key.vout, utxo);
            }
        }

        for (_, utxo) in utxos {
            Utxo::batch_put(&self.rocksdb, &mut batch, &utxo);
            undos.push(Undo::UtxoDelete(utxo.key().into_owned()));

            let utxo_key = utxo.key;
            UtxoKey::batch_put(&self.rocksdb, &mut batch, &utxo_key);
            undos.push(Undo::UtxoKeyDelete(utxo_key.key().into_owned()));
        }

        for (_, info) in infos {
            ScriptInfo::batch_put(&self.rocksdb, &mut batch, &info);
        }

        for undo in undos.iter() {
            match undo {
                Undo::UtxoPut(utxo) => {
                    Utxo::batch_delete(&self.rocksdb, &mut batch, &utxo.key());
                }
                Undo::UtxoKeyPut(vout_script) => {
                    UtxoKey::batch_delete(&self.rocksdb, &mut batch, &vout_script.key());
                }
                Undo::ScriptInfoPut(_) => {}
                Undo::UtxoDelete(_) => {}
                Undo::UtxoKeyDelete(_) => {}
                Undo::ScriptInfoDelete(_) => {}
            }
        }

        let block = Block {
            height,
            hash: U256::from_hex(&rpc_block.hash),
            prev_hash: U256::from_hex(&rpc_block.previousblockhash.unwrap_or_else(|| {
                "0000000000000000000000000000000000000000000000000000000000000000".to_string()
            })),
        };
        Block::batch_put(&self.rocksdb, &mut batch, &block);

        let block_undo = BlockUndo { height, vec: undos };
        BlockUndo::batch_put(&self.rocksdb, &mut batch, &block_undo);

        self.rocksdb.write(batch).expect("Failed to push block")
    }

    pub fn prune_until(&self, height: u64) {
        // TODO(fuxingloh): delete_range_cf isn't implemented for TransactionDB yet, unless we fork
        //  the rocksdb crate and implement it ourselves.
        let mut opts = ReadOptions::default();
        opts.set_iterate_lower_bound(bincode::serialize(&0u64).unwrap());
        opts.set_iterate_upper_bound(bincode::serialize(&height).unwrap());

        let blocks = Block::iterator(&self.rocksdb, opts, IteratorMode::Start);
        for block in blocks {
            let mut batch = WriteBatchWithTransaction::default();
            Block::batch_delete(&self.rocksdb, &mut batch, &block.key());
            BlockUndo::batch_delete(&self.rocksdb, &mut batch, &block.key());
            self.rocksdb.write(batch).expect("Failed to prune block");
            tracing::info!("Pruned block: ({}, {})", block.height, block.hash.to_hex());
        }
    }

    fn get_utxo(&self, vout: &Vout) -> Utxo {
        let vout_key = UtxoKey::read(&self.rocksdb, vout)
            .unwrap_or_else(|| panic!("UtxoKey not found {}", vout));
        Utxo::read(&self.rocksdb, &vout_key).expect("Utxo not found")
    }

    pub fn get_block(&self, height: u64) -> Option<Block> {
        Block::read(&self.rocksdb, &height)
    }

    pub fn get_script_info(&self, script: &[u8]) -> Option<ScriptInfo> {
        ScriptInfo::read(&self.rocksdb, script)
    }

    pub fn iterator_script_utxo(
        &self,
        script: &[u8],
        upper_lower_bound: Range<Option<u64>>,
    ) -> CFIterator<Utxo> {
        let mut opts = ReadOptions::default();
        opts.set_prefix_same_as_start(true);

        if let Some(lower_bound) = upper_lower_bound.start {
            let start = bincode::serialize(&(script, lower_bound)).unwrap();
            opts.set_iterate_lower_bound(start);
        }
        if let Some(upper_bound) = upper_lower_bound.end {
            let end = bincode::serialize(&(script, upper_bound)).unwrap();
            opts.set_iterate_upper_bound(end);
        }

        let prefix = bincode::serialize(&script).unwrap();
        let mode = IteratorMode::From(prefix.as_ref(), Direction::Forward);
        Utxo::iterator(&self.rocksdb, opts, mode)
    }
}
