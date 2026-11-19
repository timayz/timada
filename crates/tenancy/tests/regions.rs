mod common;

use common::*;
use evento::Evento;
use timada_tenancy::Command;

/// A region is on the list once, however often its node starts.
#[tokio::test]
async fn a_region_is_listed_once() {
    let cmd = command().await;
    assert!(cmd.load_regions().await.unwrap().is_empty());
    cmd.open_region("timada-0").await.unwrap();
    cmd.open_region("timada-0").await.unwrap();
    let regions = cmd.load_regions().await.unwrap();
    assert_eq!(regions.into_iter().collect::<Vec<_>>(), ["timada-0"]);
}

/// Each node writes under its own region, the list included: the list is
/// one stream all the same, and every node reads the whole of it.
#[tokio::test]
async fn nodes_of_different_regions_share_the_list() {
    let store = command().await.0;
    let node = |region: &str| Command(Evento::new(store.clone()).default_routing_key(region));
    node("timada-0").open_region("timada-0").await.unwrap();
    node("timada-1").open_region("timada-1").await.unwrap();
    node("timada-2").open_region("timada-2").await.unwrap();
    for region in ["timada-0", "timada-1", "timada-2"] {
        let regions = node(region).load_regions().await.unwrap();
        assert_eq!(
            regions.into_iter().collect::<Vec<_>>(),
            ["timada-0", "timada-1", "timada-2"]
        );
    }
}
