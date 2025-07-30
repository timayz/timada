use liteventd::{Aggregator, AggregatorEvent, Event, EventData, Executor, WriteError};
use liteventd_macros::AggregatorEvent;
use serde::{Deserialize, Serialize};

pub async fn load<E: Executor>(executor: &E) -> anyhow::Result<()> {
    let id = liteventd::create::<Calcul>()
        .metadata(&true)?
        .data(&Added { value: 13 })?
        .data(&Subtracted { value: 3 })?
        .data(&Multiplied { value: 3 })?
        .data(&Divided { value: 10 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.item.value, 3);

    liteventd::save(calcul, id)
        .metadata(&true)?
        .data(&Added { value: 9 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.item.value, 12);
    Ok(())
}

pub async fn version<E: Executor>(executor: &E) -> anyhow::Result<()> {
    let id = liteventd::create::<Calcul>()
        .metadata(&true)?
        .data(&Added { value: 2 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.event.version, 1);

    liteventd::save(calcul.clone(), id)
        .metadata(&true)?
        .data(&Added { value: 9 })?
        .data(&Added { value: 3 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.event.version, 3);

    let id = liteventd::create::<Calcul>()
        .metadata(&true)?
        .data(&Added { value: 2 })?
        .data(&Added { value: 32 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.event.version, 2);

    liteventd::save(calcul.clone(), id)
        .metadata(&true)?
        .data(&Added { value: 9 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.event.version, 3);

    Ok(())
}

pub async fn routing_key<E: Executor>(executor: &E) -> anyhow::Result<()> {
    let id = liteventd::create::<Calcul>()
        .metadata(&true)?
        .data(&Added { value: 2 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.event.routing_key, "default");

    liteventd::save(calcul.clone(), id)
        .routing_key("routing1")
        .metadata(&true)?
        .data(&Added { value: 9 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.event.routing_key, "default");

    let id = liteventd::create::<Calcul>()
        .routing_key("routing1")
        .metadata(&true)?
        .data(&Added { value: 2 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.event.routing_key, "routing1");

    liteventd::save(calcul.clone(), id)
        .metadata(&true)?
        .data(&Added { value: 9 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    assert_eq!(calcul.event.routing_key, "routing1");

    Ok(())
}

pub async fn invalid_original_version<E: Executor>(executor: &E) -> anyhow::Result<()> {
    let id = liteventd::create::<Calcul>()
        .metadata(&true)?
        .data(&Added { value: 2 })?
        .commit(executor)
        .await?;

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    liteventd::save(calcul.clone(), id)
        .metadata(&true)?
        .data(&Added { value: 9 })?
        .commit(executor)
        .await?;

    let res = liteventd::save(calcul, id)
        .metadata(&true)?
        .data(&Multiplied { value: 3 })?
        .commit(executor)
        .await;

    assert_eq!(
        res.map_err(|e| e.to_string()),
        Err(WriteError::InvalidOriginalVersion.to_string())
    );

    let calcul = liteventd::load::<Calcul, _>(executor, id).await?;
    liteventd::save(calcul, id)
        .metadata(&true)?
        .data(&Subtracted { value: 39 })?
        .commit(executor)
        .await?;

    Ok(())
}

pub async fn subscribe<E: Executor>(executor: &E) -> anyhow::Result<()> {
    todo!()
}

pub async fn subscribe_routing_key<E: Executor>(executor: &E) -> anyhow::Result<()> {
    todo!()
}

pub async fn subscribe_persistent<E: Executor>(executor: &E) -> anyhow::Result<()> {
    todo!()
}

pub async fn subscribe_persistent_routing_key<E: Executor>(executor: &E) -> anyhow::Result<()> {
    todo!()
}

#[derive(Debug, Serialize, Deserialize, PartialEq, AggregatorEvent)]
struct Added {
    pub value: i16,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, AggregatorEvent)]
struct Subtracted {
    pub value: i16,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, AggregatorEvent)]
struct Multiplied {
    pub value: i16,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, AggregatorEvent)]
struct Divided {
    pub value: i16,
}

type CalculEvent<D> = EventData<D, bool>;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
struct Calcul {
    pub value: i64,
}

#[liteventd_macros::aggregator]
impl Calcul {
    async fn added(&mut self, event: CalculEvent<Added>) -> anyhow::Result<()> {
        self.value += event.data.value as i64;

        Ok(())
    }

    async fn subtracted(&mut self, event: CalculEvent<Subtracted>) -> anyhow::Result<()> {
        self.value -= event.data.value as i64;

        Ok(())
    }

    async fn multiplied(&mut self, event: CalculEvent<Multiplied>) -> anyhow::Result<()> {
        self.value *= event.data.value as i64;

        Ok(())
    }

    async fn divided(&mut self, event: CalculEvent<Divided>) -> anyhow::Result<()> {
        self.value /= event.data.value as i64;

        Ok(())
    }
}
