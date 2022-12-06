use anyhow::{bail, ensure, Result};
use log::debug;
use tendermint_rpc::query::Query;
use tendermint_rpc::{Client, HttpClient, Order};

pub async fn get_transaction(tx_id: &str, url: &str) -> Result<Vec<u8>> {
    let client = HttpClient::new(url)?;
    // todo: this index key might have to be a part of the shared lib so that both the CLI and the ABCI can be in sync
    let query = Query::contains("app.tx_id", tx_id);

    let response = client
        .tx_search(query, false, 1, 1, Order::Ascending)
        .await?;

    // early return with error if no transaction has been indexed for that tx id
    ensure!(
        response.total_count > 0,
        "Transaction ID {} is invalid or has not yet been committed to the blockchain",
        tx_id
    );

    let tx_bytes: Vec<u8> = response.txs.into_iter().next().unwrap().tx.into();

    Ok(tx_bytes)
}

pub async fn broadcast(transaction: Vec<u8>, url: &str) -> Result<()> {
    let client = HttpClient::new(url).unwrap();

    let tx: tendermint::abci::Transaction = transaction.into();

    let response = client.broadcast_tx_sync(tx).await?;

    debug!("Response from CheckTx: {:?}", response);
    match response.code {
        tendermint::abci::Code::Ok => Ok(()),
        tendermint::abci::Code::Err(code) => {
            bail!("Error executing transaction {}: {}", code, response.log)
        }
    }
}

pub async fn query(query: Vec<u8>, url: &str) -> Result<Vec<u8>> {
    let client = HttpClient::new(url).unwrap();

    let response = client.abci_query(None, query, None, true).await?;

    debug!("Response from Query: {:?}", response);
    match response.code {
        tendermint::abci::Code::Ok => Ok(response.value),
        tendermint::abci::Code::Err(code) => {
            bail!("Error executing transaction {}: {}", code, response.log)
        }
    }
}
