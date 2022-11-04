# Example KV ABCI app

This app implements a basic ABCI app that uses a persistent KV store which also keeps info last block committed.

## Persistence

ABCI apps need to do some small amount of work in order to ensure some functionality when interacting with Tendermint Core.

Maintaining the last block height that we committed with the ABCI app state is one of this tasks. 

 - When Tendermint Core first connects to the ABCI app, there is a handshake that occurs where an Info message is sent to the app, and it needs to respond accordingly. One of this data points is the last block height. If it's far below what the tender mint blockchain contains, it will error out. Otherwise it will send the app the block delta. 
- [Doc reference](https://github.com/tendermint/tendermint/blob/main/spec/abci/abci++_methods.md#info)
    - Return information about the application state.
    - Used to sync Tendermint with the application  during a handshake that happens on startup or on recovery.
    - The returned app_version will be included in the Header of every block.
    - Tendermint expects last_block_app_hash and last_block_height to be updated and persisted during Commit.

In this ABCI binary, we implemented the original KV example application with RocksDB in order to ensure persistence of transactions. Furthermore, it's used to maintain the last seen block height so that the Tendermint handshake is executed properly. 

The app also indexes transactions on the blockchain and can be queried like so:

`curl http://localhost:26657/tx_search?query=%22app.key=%27somekey%27%22`


````
{
  "jsonrpc": "2.0",
  "id": -1,
  "result": {
    "txs": [
      {
        "hash": "17ED61261A5357FEE7ACDE4FAB154882A346E479AC236CFB2F22A2E8870A9C3D",
        "height": "216",
        "index": 0,
        "tx_result": {
          "code": 0,
          "data": null,
          "log": "",
          "info": "",
          "gas_wanted": "0",
          "gas_used": "0",
          "events": [
            {
              "type": "app",
              "attributes": [
                {
                  "key": "a2V5",
                  "value": "c29tZWtleQ==",
                  "index": true
                },
                {
                  "key": "aW5kZXhfa2V5",
                  "value": "aW5kZXggaXMgd29ya2luZw==",
                  "index": true
                },
                {
                  "key": "bm9pbmRleF9rZXk=",
                  "value": "aW5kZXggaXMgd29ya2luZw==",
                  "index": false
                }
              ]
            }
          ],
          "codespace": ""
        },
        "tx": "c29tZWtleT1zb21ldmFsdWU="
      },
      {
        "hash": "574D93E6298DF2E83E5C6B4DC63AE9280EB04B7589AED4EC0E7BFA8E0BC27F80",
        "height": "396",
        "index": 0,
        "tx_result": {
          "code": 0,
          "data": null,
          "log": "",
          "info": "",
          "gas_wanted": "0",
          "gas_used": "0",
          "events": [
            {
              "type": "app",
              "attributes": [
                {
                  "key": "a2V5",
                  "value": "c29tZWtleQ==",
                  "index": true
                },
                {
                  "key": "aW5kZXhfa2V5",
                  "value": "aW5kZXggaXMgd29ya2luZw==",
                  "index": true
                },
                {
                  "key": "bm9pbmRleF9rZXk=",
                  "value": "aW5kZXggaXMgd29ya2luZw==",
                  "index": false
                }
              ]
            }
          ],
          "codespace": ""
        },
        "tx": "c29tZWtleQ=="
      }
    ],
    "total_count": "2"
  }
}
````
