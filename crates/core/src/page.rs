//! A list read page after page from where the page beside it stopped,
//! never by skipping the rows before it (`OFFSET`), so a deep page costs
//! what the first one does. The list is ordered by a time and an id, most
//! recent first; a page's statement walks an index in that order from a
//! [`Cursor`], down for the older rows or up for the newer ones.

/// Where a page stands: the time (unix seconds) and the id of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub at: i64,
    pub id: String,
}

impl Cursor {
    /// As a link carries it: `{at}-{id}`.
    pub fn param(&self) -> String {
        format!("{}-{}", self.at, self.id)
    }

    /// What [`param`](Self::param) wrote; `None` for anything else.
    pub fn parse(param: &str) -> Option<Self> {
        let (at, id) = param.trim().split_once('-')?;
        Some(Self {
            at: at.parse().ok()?,
            id: id.to_owned(),
        })
    }
}

/// Which page of a list to read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PageFrom<'a> {
    /// The most recent rows.
    #[default]
    First,
    /// The rows before the cursor's.
    Older(&'a Cursor),
    /// The rows after the cursor's: the page just before.
    Newer(&'a Cursor),
}

impl<'a> PageFrom<'a> {
    /// What a list's `after` and `before` parameters ask for, `after` first.
    pub fn of(after: Option<&'a Cursor>, before: Option<&'a Cursor>) -> Self {
        match (after, before) {
            (Some(after), _) => Self::Older(after),
            (None, Some(before)) => Self::Newer(before),
            (None, None) => Self::First,
        }
    }
}

/// A page of a list, with where the pages on either side start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub rows: Vec<T>,
    /// The cursor of the newer page, `None` on the first one.
    pub newer: Option<Cursor>,
    /// The cursor of the older page, `None` on the last one.
    pub older: Option<Cursor>,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            newer: None,
            older: None,
        }
    }
}

/// Which side of a cursor a statement walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Down from it, most recent first: as the list reads.
    Older,
    /// Up from it, nearest first.
    Newer,
}

/// A page of `limit` rows at most from `from`. `read(side, cursor, n)`
/// returns up to `n` rows on `side` of `cursor` (`None`: from the top), in
/// the order it walks; `cursor_of` says where a row stands.
pub async fn read_page<T, F, Fut>(
    from: PageFrom<'_>,
    limit: i64,
    read: F,
    cursor_of: impl Fn(&T) -> Cursor,
) -> anyhow::Result<Page<T>>
where
    F: Fn(Side, Option<Cursor>, i64) -> Fut,
    Fut: Future<Output = anyhow::Result<Vec<T>>>,
{
    // One more than shown: whether there is a page beyond.
    let older_from = |cursor: Option<&Cursor>, mut rows: Vec<T>| {
        let older = (rows.len() as i64 > limit).then(|| {
            rows.truncate(limit as usize);
            cursor_of(rows.last().expect("a full page"))
        });
        Page {
            rows,
            newer: cursor.cloned(),
            older,
        }
    };
    match from {
        PageFrom::First => Ok(older_from(None, read(Side::Older, None, limit + 1).await?)),
        PageFrom::Older(cursor) => {
            let rows = read(Side::Older, Some(cursor.clone()), limit + 1).await?;
            let mut page = older_from(Some(cursor), rows);
            // The newer page starts right above this one's first row.
            page.newer = page.rows.first().map(&cursor_of).or(page.newer);
            Ok(page)
        }
        PageFrom::Newer(cursor) => {
            let mut rows = read(Side::Newer, Some(cursor.clone()), limit + 1).await?;
            if rows.len() as i64 <= limit {
                // Nothing newer beyond it: that is the first page, full.
                return Ok(older_from(None, read(Side::Older, None, limit + 1).await?));
            }
            rows.truncate(limit as usize);
            rows.reverse();
            Ok(Page {
                newer: rows.first().map(&cursor_of),
                older: rows.last().map(&cursor_of),
                rows,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_round_trips_through_a_link() {
        let cursor = Cursor {
            at: 1_731_300_000,
            id: "01jc5z-x".into(),
        };
        assert_eq!(Cursor::parse(&cursor.param()), Some(cursor));
        assert_eq!(Cursor::parse("x-1"), None);
        assert_eq!(Cursor::parse("12"), None);
    }

    /// Pages over `0..10`, the most recent being 9.
    async fn page(from: PageFrom<'_>) -> Page<i64> {
        read_page(
            from,
            3,
            |side, cursor, n| async move {
                let rows: Vec<i64> = match side {
                    Side::Older => {
                        let below = cursor.map_or(10, |c| c.at);
                        (0..below).rev().take(n as usize).collect()
                    }
                    Side::Newer => {
                        let above = cursor.map_or(-1, |c| c.at);
                        (above + 1..10).take(n as usize).collect()
                    }
                };
                Ok(rows)
            },
            |n| Cursor {
                at: *n,
                id: n.to_string(),
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn pages_walk_down_and_back_up() {
        let first = page(PageFrom::First).await;
        assert_eq!(first.rows, [9, 8, 7]);
        assert_eq!(first.newer, None);
        let second = page(PageFrom::Older(first.older.as_ref().unwrap())).await;
        assert_eq!(second.rows, [6, 5, 4]);
        let last = page(PageFrom::Older(&Cursor {
            at: 3,
            id: "3".into(),
        }))
        .await;
        assert_eq!(last.rows, [2, 1, 0]);
        assert_eq!(last.older, None);
        let back = page(PageFrom::Newer(last.newer.as_ref().unwrap())).await;
        assert_eq!(back.rows, [5, 4, 3]);
        // Above the second page there are only three rows: the first page.
        let top = page(PageFrom::Newer(second.newer.as_ref().unwrap())).await;
        assert_eq!(top, first);
    }
}
