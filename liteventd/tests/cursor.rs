use liteventd::{
    Event,
    cursor::{Args, Cursor, Edge, PageInfo, ReadResult},
};
use rand::{Rng, seq::IndexedRandom};
use std::collections::HashMap;
use ulid::Ulid;

pub fn assert_read_result(
    args: Args,
    mut data: Vec<Event>,
    result: ReadResult<Event>,
) -> anyhow::Result<()> {
    data.sort_by(|a, b| {
        if a.timestamp != b.timestamp {
            return b.timestamp.cmp(&a.timestamp);
        }

        if a.version != b.version {
            return b.version.cmp(&a.version);
        }

        b.id.cmp(&a.id)
    });

    if args.is_backward() {
        data = data.into_iter().rev().collect();
    }

    let (limit, cursor) = args.get_info();

    if let Some(cursor) = cursor.as_ref() {
        let cursor = Event::deserialize_cursor(cursor)?;
        data.retain(|event| {
            if args.is_backward() {
                event.timestamp < cursor.t
                    || (event.timestamp == cursor.t
                        && (event.version < cursor.v
                            || (event.version == cursor.v && event.id < cursor.i)))
            } else {
                event.timestamp > cursor.t
                    || (event.timestamp == cursor.t
                        && (event.version > cursor.v
                            || (event.version == cursor.v && event.id > cursor.i)))
            }
        });
    }

    let data_len = data.len();
    data = data.into_iter().take((limit + 1).into()).collect();

    let has_more = data_len > data.len();
    if has_more {
        data.pop();
    }

    let edges = data
        .into_iter()
        .map(|node| Edge {
            cursor: node
                .serialize_cursor()
                .expect("Error while serialize_cursor in assert_read_result"),
            node,
        })
        .collect::<Vec<_>>();

    let page_info = if args.is_backward() {
        let edges = edges.iter().rev().collect::<Vec<_>>();

        PageInfo {
            has_previous_page: has_more,
            start_cursor: edges.first().map(|e| e.cursor.to_owned()),
            ..Default::default()
        }
    } else {
        PageInfo {
            has_next_page: has_more,
            end_cursor: edges.last().map(|e| e.cursor.to_owned()),
            ..Default::default()
        }
    };

    assert_eq!(result.page_info, page_info);
    assert_eq!(result.edges, edges);

    Ok(())
}

pub fn get_data() -> Vec<Event> {
    let aggregate_ids = [
        Ulid::new(),
        Ulid::new(),
        Ulid::new(),
        Ulid::new(),
        Ulid::new(),
    ];

    let timestamps: Vec<u32> = vec![rand::random(), rand::random(), rand::random()];
    let mut versions: HashMap<Ulid, u16> = HashMap::new();
    let mut data = vec![];

    for _ in 0..100 {
        let mut rng = rand::rng();
        let aggregate_id = aggregate_ids
            .choose(&mut rng)
            .cloned()
            .unwrap_or_else(Ulid::new);

        let version = versions.entry(aggregate_id).or_default();
        let timestamp = if rng.random_range(0..100) < 20 {
            timestamps.choose(&mut rng).cloned()
        } else {
            None
        }
        .unwrap_or_else(|| rng.random());

        let event = Event {
            id: Ulid::new(),
            name: "MessageSent".to_owned(),
            aggregate_id,
            aggregate_type: "Message".to_owned(),
            version: version.to_owned(),
            routing_key: "".to_owned(),
            timestamp,
            data: Default::default(),
            metadata: Default::default(),
        };

        data.push(event);

        *version += 1;
    }

    data
}
