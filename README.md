# OXTU

OXTU (UTXO in reverse) is a space-efficient UTXO set for Bitcoin (and bitcoin-like) blockchains.
It is experimental software with no warranty.

## Available RPC

The RPC attempts to model the Bitcoin Core RPC as much as possible.
However, due to the nature of OXTU being wallet agnostic, the RPC will not work as the same as Bitcoin Core RPC.

- `listunspent` (address=String, {minconf, maxconf, count})
- `getaddressinfo` (address=String)
- `_probe` (name=liveness|readiness|startup) for K8s.

## Usage

A `compose.yml` file is provided below as an example on how to run OXTU together with a Bitcoin Core.
The Bitcoin Core will be used as the source of truth for the UTXO set.
Three `BITCOIND_RPC_*` environment variables are required to connect to the Bitcoin Core.

```yaml
version: '3.8'

services:
  bitcoind:
    image: docker.io/kylemanna/bitcoind:latest
    environment:
      - RPCUSER=oxtu
      - RPCPASSWORD=oxtu
    volumes:
      - bitcoind:/bitcoin/.bitcoin

  oxtu:
    image: ghcr.io/fuxingloh/oxtu:latest
    ports:
      - "3000:3000"
    environment:
      - BITCOIND_RPC_URL=http://bitcoind:8332
      - BITCOIND_RPC_USERNAME=oxtu
      - BITCOIND_RPC_PASSWORD=oxtu
    volumes:
      - oxtu:/oxtu/.oxtu
    depends_on:
      - bitcoind

volumes:
  bitcoind:
  oxtu:
```

## OXTU Design

### Connecting Blocks

OXTU uses the same "connecting block" mechanism as Bitcoin Core.
The connected block is a block where `prev_hash` is the `hash` of the stored block.
Otherwise, the stored block will be forked out using `BlockUndo` to revert the changes.
JSON-RPC is used to communicate with the underlying bitcoin node.

### Pruning Blocks

Every 10,000 blocks, OXTU will prune Block and BlockUndo, those are not needed anymore and are only used for reorgs.

### RocksDB

OXTU uses RocksDB as the storage engine. The data, by default is stored in the `/oxtu/.oxtu/data` directory.
This is chosen over single-file options to take advantage of layered storage.

## License

MIT