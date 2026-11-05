mod common;

use common::*;
use timada_pages::{
    AnnouncementStatus, MAX_ANNOUNCEMENTS, NewAnnouncement, PageError, render_announcement,
};

fn message(text: &str) -> NewAnnouncement {
    NewAnnouncement {
        text: text.to_owned(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_message_is_added_shown_on_one_line() {
    let pages = command().await;
    assert!(pages.announcements().await.unwrap().is_empty());
    let id = pages
        .add_announcement(
            ALICE,
            message("  -10% maintenant\r\navec le code **BIENVENUE10** "),
        )
        .await
        .unwrap();
    let all = pages.announcements().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, id);
    assert_eq!(all[0].text, "-10% maintenant avec le code **BIENVENUE10**");
    assert_eq!(all[0].status(0), AnnouncementStatus::Live);
    assert_eq!(
        render_announcement(&all[0].text),
        "-10% maintenant avec le code <strong>BIENVENUE10</strong>"
    );
}

#[tokio::test]
async fn a_message_is_checked() {
    let pages = command().await;
    for (cmd, expected) in [
        (message("   "), "announcement_invalid"),
        (message(&"x".repeat(201)), "announcement_invalid"),
        (message("[a](/x) et [b](/y)"), "announcement_invalid_link"),
        (message("[a](javascript:x)"), "announcement_invalid_link"),
        (
            NewAnnouncement {
                text: "Soldes".into(),
                starts_at: Some(200),
                ends_at: Some(100),
            },
            "announcement_invalid_window",
        ),
    ] {
        let refused = pages.add_announcement(ALICE, cmd).await.unwrap_err();
        assert_eq!(refused.code(), Some(expected), "{refused:?}");
    }
    assert!(pages.announcements().await.unwrap().is_empty());
}

#[tokio::test]
async fn the_window_decides_when_it_shows() {
    let pages = command().await;
    let id = pages
        .add_announcement(
            ALICE,
            NewAnnouncement {
                text: "Soldes d'été".into(),
                starts_at: Some(100),
                ends_at: Some(200),
            },
        )
        .await
        .unwrap();
    let all = pages.announcements().await.unwrap();
    assert_eq!(all[0].status(99), AnnouncementStatus::Scheduled);
    assert_eq!(all[0].status(100), AnnouncementStatus::Live);
    assert_eq!(all[0].status(200), AnnouncementStatus::Ended);

    pages
        .change_announcement(ALICE, &id, message("Soldes d'été"))
        .await
        .unwrap();
    let all = pages.announcements().await.unwrap();
    assert_eq!((all[0].starts_at, all[0].ends_at), (None, None));
    assert_eq!(all[0].status(u64::MAX), AnnouncementStatus::Live);
}

#[tokio::test]
async fn hidden_shown_moved_removed() {
    let pages = command().await;
    let a = pages.add_announcement(ALICE, message("A")).await.unwrap();
    let b = pages.add_announcement(ALICE, message("B")).await.unwrap();
    let c = pages.add_announcement(ALICE, message("C")).await.unwrap();
    let order = async || {
        pages
            .announcements()
            .await
            .unwrap()
            .into_iter()
            .map(|announcement| announcement.text)
            .collect::<Vec<_>>()
    };

    pages.move_announcement(ALICE, &c, 0).await.unwrap();
    assert_eq!(order().await, ["C", "A", "B"]);
    pages.move_announcement(ALICE, &c, 99).await.unwrap();
    assert_eq!(order().await, ["A", "B", "C"]);

    pages.hide_announcement(ALICE, &b).await.unwrap();
    pages.hide_announcement(ALICE, &b).await.unwrap();
    let all = pages.announcements().await.unwrap();
    assert_eq!(all[1].status(0), AnnouncementStatus::Hidden);
    pages.show_announcement(ALICE, &b).await.unwrap();
    assert!(pages.announcements().await.unwrap()[1].shown);

    pages.remove_announcement(ALICE, &a).await.unwrap();
    assert_eq!(order().await, ["B", "C"]);
    let gone = pages.hide_announcement(ALICE, &a).await;
    assert!(
        matches!(gone, Err(PageError::AnnouncementNotFound)),
        "{gone:?}"
    );
}

#[tokio::test]
async fn the_bar_holds_ten() {
    let pages = command().await;
    for n in 0..MAX_ANNOUNCEMENTS {
        pages
            .add_announcement(ALICE, message(&format!("Message {n}")))
            .await
            .unwrap();
    }
    let full = pages.add_announcement(ALICE, message("One more")).await;
    assert!(
        matches!(full, Err(PageError::TooManyAnnouncements)),
        "{full:?}"
    );
}
