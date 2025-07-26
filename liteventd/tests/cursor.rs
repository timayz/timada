use liteventd::{
    Event,
    cursor::{Args, Cursor, Edge, Order, PageInfo, ReadResult},
};
use rand::{Rng, seq::IndexedRandom};
use std::collections::HashMap;
use ulid::Ulid;

// @TODO: Create Vec reader logic in cursor.rs and add static data tests than use it here by reading
// data vec and replace with vec reader result
pub fn assert_read_result(
    args: Args,
    order: Order,
    mut data: Vec<Event>,
    result: ReadResult<Event>,
) -> anyhow::Result<()> {
    let is_order_desc = matches!(
        (&order, args.is_backward()),
        (Order::Asc, true) | (Order::Desc, false)
    );

    if !is_order_desc {
        data.sort_by(|a, b| {
            if a.timestamp != b.timestamp {
                return a.timestamp.cmp(&b.timestamp);
            }

            if a.version != b.version {
                return a.version.cmp(&b.version);
            }

            a.id.cmp(&b.id)
        });
    } else {
        data.sort_by(|a, b| {
            if a.timestamp != b.timestamp {
                return b.timestamp.cmp(&a.timestamp);
            }

            if a.version != b.version {
                return b.version.cmp(&a.version);
            }

            b.id.cmp(&a.id)
        });
    }

    let (limit, cursor) = args.get_info();

    if let Some(cursor) = cursor.as_ref() {
        let cursor = Event::deserialize_cursor(cursor)?;
        data.retain(|event| {
            if is_order_desc {
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

    let mut edges = data
        .into_iter()
        .map(|node| Edge {
            cursor: node
                .serialize_cursor()
                .expect("Error while serialize_cursor in assert_read_result"),
            node,
        })
        .collect::<Vec<_>>();

    if args.is_backward() {
        edges = edges.into_iter().rev().collect();
    }

    let page_info = if args.is_backward() {
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

    for _ in 0..10 {
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
        .unwrap_or_else(|| rng.random()) as u64;

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
