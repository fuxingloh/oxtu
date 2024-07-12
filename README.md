# OXTU

OXTU (UTXO in reverse) is a space-efficient UTXO set for Bitcoin (and bitcoin-like) blockchains.
It is experimental software with no warranty.

## Available RPC

The RPC attempts to model the Bitcoin Core RPC as much as possible.
However, due to the nature of OXTU being wallet agnostic, the RPC will not work as the same as Bitcoin Core RPC.

- `listunspent` (address=String, {minconf, maxconf, count})
- `getaddressinfo` (address=String)
- `_probe` (name=liveness|readiness|startup) for K8s.

## License

MIT