//! 전송 전 **같은 이름 확인** 흐름 (FR-55) — `ui::app`의 자식 모듈.
//!
//! 받기·올리기가 대상에 이미 있는 이름을 만나면 사용자에게 먼저 묻는다. 확인이 끝나기
//! 전에는 아무것도 큐에 넣지 않아, 고른 `취소`가 절반만 취소되지 않게 한다.
//!
//! **부모(`ui::app`)의 자식으로 둔 이유**: 이 흐름은 `ExplorerApp`의 private 필드
//! (`pending_conflicts`·`conflict_lists`·`conflict_dialog` 등)를 직접 만진다. 형제 모듈로
//! 두면 그 필드를 `pub(crate)`로 넓혀야 하지만, 자식이면 가시성을 그대로 두고 나눌 수 있다.
//!
//! 원격 처리와 서로 부른다 — 확인 결과가 서버 조회로 오는 갈래가 있어 `app.rs`의
//! `poll_remote`가 `settle_conflict`를 부르고, 이쪽은 `site_connection`·`request_tree`를 쓴다.

use super::ExplorerApp;
use crate::remote::connection::{ConnCommand, ConnectionId, ListSource, TransferDirection};
use crate::remote::transfer;
use crate::remote::types::SiteId;
use crate::ui::list_common::{self, ConflictChoice, DragItem, DropOutcome, DropTarget};
use crate::ui::remote_menu::{self, DialogOutcome};
use eframe::egui;
use std::path::PathBuf;

impl ExplorerApp {
    /// 전송을 시작한다 — `apply_drop`의 **유일한 앞문**이다 (FR-55).
    ///
    /// 대상에 같은 이름이 이미 있으면 먼저 물어본다. 확인이 끝나기 전에는 **아무것도 큐에
    /// 넣지 않는다** — 그래야 사용자가 고른 `취소`가 절반만 취소되지 않는다.
    /// 원격 메뉴의 받기·올리기와 끌어다 놓기가 모두 이 문을 지난다
    pub(super) fn start_transfer(&mut self, drop: DropOutcome) {
        match drop.target.clone() {
            // 받는 곳은 파일시스템이다 — 존재 확인은 워커 스레드가 한다
            // (AGENTS: UI 스레드에서 블로킹 I/O 금지)
            DropTarget::Local(dir) => {
                let names = local_landing_names(&drop);
                if names.is_empty() {
                    return;
                }
                let id = self.next_conflict;
                self.next_conflict += 1;
                self.pending_conflicts.push(ConflictCheck { id, drop });
                let tx = self.conflict_tx.clone();
                let wake = self.repaint.clone();
                std::thread::spawn(move || {
                    // 받는 곳은 Windows라 대소문자를 가리지 않는다 — `A.TXT`가 있으면
                    // `a.txt`도 실제로 덮인다(D5)
                    let existing: Vec<String> = std::fs::read_dir(&dir)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect();
                    if tx
                        .send((id, conflict_names(&names, &existing, true)))
                        .is_ok()
                    {
                        wake();
                    }
                });
            }
            // 올리는 곳은 서버다 — 대상 폴더를 조회해 이름을 대조한다
            DropTarget::Remote { site, dir } => {
                let names: Vec<String> = drop
                    .items
                    .iter()
                    .filter(|item| list_common::drop_direction(item, &drop.target).is_some())
                    .map(DragItem::name)
                    .collect();
                // 연결이 없으면 물어볼 길이 없다 — 확인을 건너뛰고 보낸다 (D10)
                let Some(conn) = self.site_connection(site).filter(|_| !names.is_empty()) else {
                    self.apply_drop(drop);
                    return;
                };
                let id = self.next_conflict;
                self.next_conflict += 1;
                self.pending_conflicts.push(ConflictCheck { id, drop });
                // **등록과 발송이 같은 확인 번호를 쓴다** — 종전에는 기준값을 더한 세대로
                // 보내고 답에서 다시 빼 되찾았는데, 한쪽만 고쳐지면 답이 와도 찾지 못해
                // 그 전송이 대화도 못 뜨고 큐에도 못 들어간 채 사라졌다. 이제 출처가
                // 확인 번호를 그대로 나르므로 더하고 뺄 자리가 없다
                self.conflict_lists.insert(id, (conn, names));
                self.manager.send(
                    conn,
                    ConnCommand::List {
                        source: ListSource::Conflict { id },
                        path: dir,
                        quiet: false,
                    },
                );
            }
        }
    }

    /// 확인 결과가 온 전송을 처리한다 (FR-55).
    ///
    /// 겹치는 것이 없으면 그대로 큐로 보내고, 있으면 물어볼 것으로 쌓아 둔다.
    ///
    /// **대화는 한 번에 하나만** 뜨므로 그 사이에 도착한 결과는 `conflict_queue`에서 차례를
    /// 기다린다 — 워커는 답을 한 번만 보내니 여기서 그 답까지 함께 들고 있어야 한다.
    /// 들지 않고 되돌리면 그 전송은 대화도 못 뜨고 큐에도 못 들어간 채 사라진다
    pub(super) fn drain_conflict_checks(&mut self) {
        while let Ok((id, conflicts)) = self.conflict_rx.try_recv() {
            self.settle_conflict(id, conflicts);
        }
        // 대화 자리가 비어 있으면 기다리던 것 중 먼저 온 것을 올린다
        if self.conflict_dialog.is_none() && !self.conflict_queue.is_empty() {
            self.conflict_dialog = Some(self.conflict_queue.remove(0));
        }
    }

    /// 그 사이트와 오가는 확인 대기를 **버린다** — 사이드바에서 사이트를 지웠을 때 (FR-29).
    ///
    /// 아래 `abandon_conflict_lists`와 겨냥하는 것이 다르다. 그쪽은 답이 오지 않게 된 확인을
    /// **겹침 없음으로 보고 큐에 보내는** 길인데(연결만 사라졌을 뿐 사용자는 그 전송을 원한다),
    /// 여기서는 사이트째 거두는 것이라 **보낼 곳 자체가 없다**. 그래서 `settle_conflict`를
    /// 부르지 않는다 — 부르면 방금 비운 큐에 그 사이트의 전송이 되살아나 연결별 탭이 다시 선다.
    ///
    /// **큐를 비우기 전에 부른다** — 순서가 뒤집히면 위와 같은 되살아남이 생긴다
    pub(super) fn drop_site_conflicts(&mut self, site: SiteId) {
        self.pending_conflicts
            .retain(|check| conflict_site(&check.drop) != Some(site));
        self.conflict_queue
            .retain(|(check, _)| conflict_site(&check.drop) != Some(site));
        if self
            .conflict_dialog
            .as_ref()
            .is_some_and(|(check, _)| conflict_site(&check.drop) == Some(site))
        {
            // 물을 상대가 사라졌다 — 대화를 내린다
            self.conflict_dialog = None;
        }
        // 서버에 물어 둔 조회도 거둔다. 사이트 하나가 연결 셋을 쓰므로(FR-37) 그 사이트의
        // 연결로 물은 것을 모두 고른다 — 지울 것을 먼저 모으는 이유는 표를 고치는 동안
        // 연결 관리자를 함께 빌릴 수 없기 때문이다
        let stale: Vec<u64> = self
            .conflict_lists
            .iter()
            .filter(|(_, (asked, _))| {
                self.manager
                    .get(*asked)
                    .is_some_and(|connection| connection.site == site)
            })
            .map(|(generation, _)| *generation)
            .collect();
        for generation in stale {
            self.conflict_lists.remove(&generation);
        }
    }

    /// 그 연결에 물어 둔 확인을 모두 포기한다 (FR-55).
    ///
    /// 연결이 끊기면 답이 오지 않는다 — 붙잡아 두면 그 전송은 대화도 못 뜨고 큐에도 못
    /// 들어간 채 남는다. 확인은 안전장치이지 관문이 아니므로 겹침 없음으로 보고 보낸다 (D10)
    pub(super) fn abandon_conflict_lists(&mut self, conn: ConnectionId) {
        // **그 연결로** 물어 둔 것만 거둔다 — 같은 사이트의 다른 연결에 물어 둔 확인은
        // 그대로 답을 기다린다(사이트 하나가 연결 셋을 쓴다 — FR-37)
        // 키가 곧 확인 번호다 — 종전에는 기준값을 더한 세대라 여기서 다시 빼야 했다
        let abandoned: Vec<u64> = self
            .conflict_lists
            .iter()
            .filter(|(_, (asked, _))| *asked == conn)
            .map(|(id, _)| *id)
            .collect();
        for id in abandoned {
            self.conflict_lists.remove(&id);
            self.settle_conflict(id, Vec::new());
        }
    }

    /// 확인이 끝난 전송 하나를 처리한다 — 받기(워커)와 올리기(서버 조회)가 함께 쓴다 (FR-55).
    ///
    /// **어디서 확인했는지는 여기서 상관없다** — 겹친 이름 목록만 있으면 그 다음 일은 같다.
    /// 대화로 올리는 것은 `drain_conflict_checks`가 프레임마다 한 번 한다
    pub(super) fn settle_conflict(&mut self, id: u64, conflicts: Vec<String>) {
        let Some(at) = self.pending_conflicts.iter().position(|c| c.id == id) else {
            return;
        };
        let check = self.pending_conflicts.remove(at);
        match apply_conflict_choice(check.drop.clone(), &conflicts, ConflictDecision::NotAsked) {
            // 겹치는 것이 없다 — 물을 것도 없이 보낸다
            Some(drop) => self.apply_drop(drop),
            None => self.conflict_queue.push((check, conflicts)),
        }
    }

    /// 같은 이름 확인 대화를 그리고 고른 대로 처리한다 (FR-55)
    pub(super) fn show_conflict_dialog(&mut self, ctx: &egui::Context) {
        let Some((check, conflicts)) = self.conflict_dialog.take() else {
            return;
        };
        // 목록에 폴더 표시를 붙이려면 원래 항목에서 종류를 되찾아야 한다
        let names: Vec<(String, bool)> = conflicts
            .iter()
            .map(|name| {
                let is_dir = check
                    .drop
                    .items
                    .iter()
                    .any(|item| item.is_dir() && item.name() == *name);
                (name.clone(), is_dir)
            })
            .collect();
        match remote_menu::show_conflict_dialog(ctx, &names) {
            DialogOutcome::Pending => self.conflict_dialog = Some((check, conflicts)),
            // 취소 — 이 전송은 아무것도 큐에 넣지 않는다 (D6)
            DialogOutcome::Cancelled => {}
            DialogOutcome::Confirmed(choice) => {
                if let Some(drop) = apply_conflict_choice(check.drop, &conflicts, choice.into()) {
                    self.apply_drop(drop);
                }
            }
        }
    }

    /// 끌어다 놓은 것을 전송 큐에 넣는다 (FR-38).
    ///
    /// **로컬 → 원격은 올리기, 원격 → 로컬은 받기**다. 여기까지 오는 것은 그 둘뿐이다 —
    /// 로컬끼리는 셸 복사라 `ui::app`이 앞에서 갈라 보내고(FR-60), 원격끼리는 PRD
    /// Out of Scope다. 그래도 걸러 두는 이유는 이 함수가 앞문이기 때문이다.
    /// 폴더는 파일 단위로 펼쳐 넣는다(T17 규약): 로컬은 그 자리에서, 원격은 워커에 훑기를 맡긴다
    fn apply_drop(&mut self, drop: DropOutcome) {
        match &drop.target {
            DropTarget::Remote { site, dir } => {
                // 폴더를 펼치는 것은 디렉터리를 재귀로 읽는 일이라 **워커 스레드**가 한다
                // (AGENTS: UI 스레드 블로킹 I/O 금지 — 큰 폴더면 프레임이 멈춘다).
                // 결과는 채널로 받아 다음 프레임에 큐로 옮긴다 (`DirLoad`와 같은 방식)
                let roots: Vec<PathBuf> = drop
                    .items
                    .iter()
                    .filter(|item| list_common::drop_direction(item, &drop.target).is_some())
                    .filter_map(|item| match item {
                        DragItem::Local { path, .. } => Some(path.clone()),
                        DragItem::Remote { .. } | DragItem::Zip { .. } => None,
                    })
                    .collect();
                if roots.is_empty() {
                    return;
                }
                let tx = self.expand_tx.clone();
                self.expanding += 1;
                let (site, dir) = (*site, dir.clone());
                let wake = self.repaint.clone();
                std::thread::spawn(move || {
                    let mut files = Vec::new();
                    let mut skipped = 0;
                    for root in roots {
                        let expanded = transfer::expand_for_transfer(&root);
                        skipped += expanded.skipped;
                        for (path, relative) in expanded.files {
                            let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                            files.push((path, dir.join(&relative), size));
                        }
                    }
                    if tx.send((site, files, skipped)).is_ok() {
                        wake();
                    }
                });
            }
            DropTarget::Local(local_dir) => {
                // zip 항목은 워커가 그 폴더에 푼다 — 전송 큐에 들지 않는다 (FR-78)
                if let Some(picks) = list_common::zip_picks(&drop) {
                    self.start_zip_extract(picks);
                    return;
                }
                let Some(site) = drop.source_site else {
                    return;
                };
                for item in &drop.items {
                    // 종류가 같으면(로컬 → 로컬) 아무 일도 하지 않는다
                    if list_common::drop_direction(item, &drop.target).is_none() {
                        continue;
                    }
                    let DragItem::Remote { path, is_dir, size } = item else {
                        continue;
                    };
                    if *is_dir {
                        self.request_tree(site, path.clone(), local_dir.clone());
                        continue;
                    }
                    self.queue.enqueue(
                        site,
                        TransferDirection::Download,
                        local_dir.join(item.name()),
                        path.clone(),
                        *size,
                    );
                }
            }
        }
    }
}

/// 로컬 폴더에 놓여 그 자리에 생길 최상위 이름들 — 같은 이름 확인(FR-55)이 견줄 것이다.
///
/// 받기(원격 → 로컬)와 zip 풀기(FR-78)가 센다 — 로컬끼리는 셸 복사라 여기 오지 않는다
pub(super) fn local_landing_names(drop: &DropOutcome) -> Vec<String> {
    drop.items
        .iter()
        .filter(|item| {
            list_common::drop_direction(item, &drop.target).is_some()
                || matches!(item, DragItem::Zip { .. })
        })
        .map(DragItem::name)
        .collect()
}

/// 고른 최상위 항목 중 **대상에 이미 있는 이름**들 (FR-55).
///
/// 판정 단위가 최상위 항목인 이유(D4): 대상 폴더 목록을 한 번만 읽으면 되어 확인 대화가
/// 곧바로 뜬다. 폴더는 통째로 덮어쓰기·건너뛰기가 되고, 그 안까지 파고들지 않는다.
///
/// `ignore_case`는 **받는 쪽 파일시스템의 규칙**이다 — 받기는 Windows라 참(대소문자를 가려
/// 봐야 실제로는 덮인다), 올리기는 대개 POSIX라 거짓(가리지 않으면 헛경고가 난다)
pub(super) fn conflict_names(
    names: &[String],
    existing: &[String],
    ignore_case: bool,
) -> Vec<String> {
    let same = |a: &str, b: &str| {
        if ignore_case {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    names
        .iter()
        .filter(|name| existing.iter().any(|had| same(name, had)))
        .cloned()
        .collect()
}

/// 같은 이름 확인이 **어느 단계까지 왔는지**.
///
/// `Option<ConflictChoice>`로 들면 `None`이 「아직 묻기 전」을 뜻하게 되는데, 읽는 사람은
/// 그것을 「고르지 않음」이나 「취소」로 읽기 쉽다. 세 상태를 이름으로 세워 그 겹침을 없앤다.
/// **취소는 여기 없다** — 취소하면 호출부가 이 함수를 아예 부르지 않는다 (D6)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConflictDecision {
    /// 겹침을 확인만 했고 아직 사용자에게 묻지 않았다
    NotAsked,
    /// 있던 것을 덮어쓰고 전부 보낸다
    Overwrite,
    /// 겹치는 것만 빼고 나머지를 보낸다
    Skip,
}

impl From<ConflictChoice> for ConflictDecision {
    fn from(choice: ConflictChoice) -> ConflictDecision {
        match choice {
            ConflictChoice::Overwrite => ConflictDecision::Overwrite,
            ConflictChoice::Skip => ConflictDecision::Skip,
        }
    }
}

/// 확인 결과와 사용자의 결정으로 **실제로 큐에 넣을 것**을 정한다 (FR-55).
///
/// D6의 계약을 이 한 함수에 담는다 — 아직 묻지 않았는데 충돌이 있으면 `None`을 돌려주어
/// **아무것도 큐에 들어가지 않게** 한다. 취소는 호출부가 아예 부르지 않는다
pub(super) fn apply_conflict_choice(
    drop: DropOutcome,
    conflicts: &[String],
    decision: ConflictDecision,
) -> Option<DropOutcome> {
    match decision {
        // 아직 묻기 전 — 겹치는 것이 없을 때만 그대로 보낸다
        ConflictDecision::NotAsked if conflicts.is_empty() => Some(drop),
        ConflictDecision::NotAsked => None,
        ConflictDecision::Overwrite => Some(drop),
        ConflictDecision::Skip => {
            let items: Vec<DragItem> = drop
                .items
                .into_iter()
                .filter(|item| !conflicts.contains(&item.name()))
                .collect();
            // 겹치는 것을 빼고 나니 남는 것이 없으면 보낼 일도 없다
            (!items.is_empty()).then_some(DropOutcome { items, ..drop })
        }
    }
}

/// 이 전송이 어느 사이트와 오가는가 — **올리기는 놓은 곳, 받기는 끌어온 곳**이다.
///
/// 로컬끼리의 드롭에는 사이트가 없다(`None`) — 그것은 셸 복사라 이 흐름에 오지도 않지만,
/// 사이트로 거두는 쪽에서 `None`을 대상으로 삼지 않게 여기서 갈라 둔다
fn conflict_site(drop: &DropOutcome) -> Option<SiteId> {
    match &drop.target {
        DropTarget::Remote { site, .. } => Some(*site),
        DropTarget::Local(_) => drop.source_site,
    }
}

/// 같은 이름 확인을 기다리는 전송 한 벌 (FR-55)
pub(super) struct ConflictCheck {
    /// 이 확인의 번호 — 워커·서버 답이 어느 조작의 것인지 잇는다
    id: u64,
    drop: DropOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::types::{RemotePath, SiteId};

    /// 원격에서 받아 오는 전송 한 벌 — 이름만 다른 파일 셋
    fn 받기_전송(names: &[&str]) -> DropOutcome {
        DropOutcome {
            items: names
                .iter()
                .map(|name| DragItem::Remote {
                    path: RemotePath::new(&format!("/pub/{name}")),
                    is_dir: false,
                    size: 10,
                })
                .collect(),
            source_site: Some(SiteId(1)),
            target: DropTarget::Local(PathBuf::from(r"D:\받은 파일")),
        }
    }

    fn 이름들(drop: &DropOutcome) -> Vec<String> {
        drop.items.iter().map(DragItem::name).collect()
    }

    #[test]
    fn 확인_대기의_사이트는_방향에_따라_다른_자리에서_온다() {
        // 사이드바에서 사이트를 지울 때 어떤 확인을 버릴지 이 판정이 정한다 (FR-29).
        // 받기는 **끌어온 곳**, 올리기는 **놓은 곳**이 그 사이트다
        let 받기 = 받기_전송(&["a.txt"]);
        assert_eq!(conflict_site(&받기), Some(SiteId(1)));

        let 올리기 = DropOutcome {
            items: vec![DragItem::Local {
                path: PathBuf::from(r"D:\보낼 파일\a.txt"),
                is_dir: false,
            }],
            source_site: None,
            target: DropTarget::Remote {
                site: SiteId(2),
                dir: RemotePath::new("/pub"),
            },
        };
        assert_eq!(conflict_site(&올리기), Some(SiteId(2)));

        // 로컬끼리는 사이트가 없다 — 사이트로 거두는 쪽이 대상으로 삼으면 안 된다
        let 로컬끼리 = DropOutcome {
            items: vec![DragItem::Local {
                path: PathBuf::from(r"D:\a.txt"),
                is_dir: false,
            }],
            source_site: None,
            target: DropTarget::Local(PathBuf::from(r"D:\받은 파일")),
        };
        assert_eq!(conflict_site(&로컬끼리), None);
    }

    #[test]
    fn 받는_쪽은_대소문자를_가리지_않는다() {
        // Windows는 A.TXT가 있으면 a.txt도 실제로 덮인다 — 가려 보면 경고를 놓친다 (D5)
        let 고른_것 = ["report.zip".to_owned(), "a.txt".to_owned()];
        let 이미 = ["REPORT.ZIP".to_owned(), "b.txt".to_owned()];
        assert_eq!(conflict_names(&고른_것, &이미, true), vec!["report.zip"]);
        // 올리는 쪽(POSIX)은 가린다 — 가리지 않으면 헛경고가 난다
        assert!(conflict_names(&고른_것, &이미, false).is_empty());
    }

    #[test]
    fn 묻기_전에는_겹치는_것이_있으면_아무것도_보내지_않는다() {
        // D6 — 취소가 절반만 취소되지 않게 하는 계약이다
        let drop = 받기_전송(&["report.zip", "a.txt", "b.txt"]);
        let 겹침 = vec!["report.zip".to_owned()];
        assert!(apply_conflict_choice(drop.clone(), &겹침, ConflictDecision::NotAsked).is_none());
        // 겹치는 것이 없으면 묻지 않고 그대로 간다
        let 그대로 = apply_conflict_choice(drop.clone(), &[], ConflictDecision::NotAsked)
            .expect("보낼 것이 있다");
        assert_eq!(이름들(&그대로).len(), 3);
    }

    #[test]
    fn 덮어쓰기는_전부_건너뛰기는_겹치는_것만_뺀다() {
        let drop = 받기_전송(&["report.zip", "a.txt", "b.txt"]);
        let 겹침 = vec!["report.zip".to_owned()];

        let 덮어쓰기 = apply_conflict_choice(drop.clone(), &겹침, ConflictDecision::Overwrite)
            .expect("덮어쓰면 전부 간다");
        assert_eq!(이름들(&덮어쓰기).len(), 3);

        let 건너뛰기 = apply_conflict_choice(drop.clone(), &겹침, ConflictDecision::Skip)
            .expect("나머지는 간다");
        assert_eq!(이름들(&건너뛰기), vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn 건너뛰어_남는_것이_없으면_보내지_않는다() {
        let drop = 받기_전송(&["report.zip"]);
        let 겹침 = vec!["report.zip".to_owned()];
        assert!(apply_conflict_choice(drop, &겹침, ConflictDecision::Skip).is_none());
    }

    #[test]
    fn 확인_결과는_대화가_떠_있어도_유실되지_않는다() {
        // 회귀 — 되돌려 넣을 때 계산된 겹침 목록을 버리면 워커가 답을 다시 보내지 않아
        // 그 전송은 대화도 못 뜨고 큐에도 못 들어간 채 사라졌다 (리뷰 B1)
        let 대기: Vec<(ConflictCheck, Vec<String>)> = vec![
            (
                ConflictCheck {
                    id: 1,
                    drop: 받기_전송(&["a.txt"]),
                },
                vec!["a.txt".to_owned()],
            ),
            (
                ConflictCheck {
                    id: 2,
                    drop: 받기_전송(&["b.txt"]),
                },
                vec!["b.txt".to_owned()],
            ),
        ];
        // 대기열에 든 것은 겹침 목록을 **함께** 들고 있어야 다음 차례에 그대로 물어볼 수 있다
        for (check, conflicts) in &대기 {
            assert!(
                !conflicts.is_empty(),
                "겹침 목록이 비어 대화를 띄울 수 없다"
            );
            assert!(
                apply_conflict_choice(check.drop.clone(), conflicts, ConflictDecision::NotAsked)
                    .is_none(),
                "겹치는데도 묻지 않고 통과했다"
            );
            let 덮어쓰기 =
                apply_conflict_choice(check.drop.clone(), conflicts, ConflictDecision::Overwrite);
            assert!(덮어쓰기.is_some(), "차례가 와도 보낼 수 없는 상태다");
        }
    }

    #[test]
    fn 올리는_쪽은_대소문자를_가린다() {
        // 원격은 대개 POSIX다 — 가리지 않으면 Setup.exe 때문에 setup.exe가 헛경고를 받는다 (D5)
        let 올릴_것 = ["setup.exe".to_owned(), "data.bin".to_owned()];
        let 원격 = ["setup.exe".to_owned(), "log".to_owned()];
        assert_eq!(conflict_names(&올릴_것, &원격, false), vec!["setup.exe"]);

        let 대문자만 = ["Setup.exe".to_owned()];
        assert!(
            conflict_names(&올릴_것, &대문자만, false).is_empty(),
            "대소문자가 다른 이름을 겹친 것으로 봤다"
        );
    }

    #[test]
    fn 확인_번호는_출처가_그대로_실어_나른다() {
        // 회귀 — 종전에는 등록을 확인 번호로, 발송을 기준값을 더한 세대로 하는 바람에
        // 답이 와도 찾지 못했다(올리기가 대화도 못 뜨고 큐에도 못 들어간 채 사라졌다).
        // 이제 출처가 그 번호를 그대로 나르므로 더하고 뺄 자리 자체가 없다
        for id in [0, 1, 7, 4096, u32::MAX as u64] {
            let ListSource::Conflict { id: 되찾은 } = (ListSource::Conflict { id }) else {
                panic!("확인 출처가 다른 갈래로 바뀌었다");
            };
            assert_eq!(되찾은, id, "출처에서 확인 번호를 되찾지 못했다");
        }
        // 서로 다른 확인은 서로 다른 출처다 — 같으면 한쪽 답을 다른 쪽이 가져간다
        assert_ne!(
            ListSource::Conflict { id: 1 },
            ListSource::Conflict { id: 2 }
        );
        // 갈래가 다르면 번호가 같아도 서로 다른 출처다 (종전의 번호 공간 분할이 하던 일)
        assert_ne!(ListSource::Conflict { id: 7 }, ListSource::Tree { seq: 7 });
    }

    fn zip_풀기(names: &[&str]) -> DropOutcome {
        DropOutcome {
            items: names
                .iter()
                .map(|name| DragItem::Zip {
                    archive: PathBuf::from(r"D:\자료.zip"),
                    path: format!("문서/{name}"),
                    is_dir: false,
                })
                .collect(),
            source_site: None,
            target: DropTarget::Local(PathBuf::from(r"D:\받은 파일")),
        }
    }

    #[test]
    fn zip_항목도_같은_이름이면_묻는다() {
        // FR-78 — zip 항목을 로컬에 풀 때도 받기와 같은 확인을 거친다: 대조할 이름에 들어가야 한다
        let 풀기 = zip_풀기(&["a.txt", "b.txt"]);
        assert_eq!(local_landing_names(&풀기), vec!["a.txt", "b.txt"]);
        let 겹침 = conflict_names(&local_landing_names(&풀기), &["A.TXT".to_owned()], true);
        assert_eq!(겹침, vec!["a.txt"]);
        assert_eq!(
            apply_conflict_choice(풀기, &겹침, ConflictDecision::NotAsked),
            None,
            "겹치는데 묻지 않고 풀었다"
        );
    }

    #[test]
    fn zip_항목_건너뛰기는_겹친_것만_뺀다() {
        let 풀기 = zip_풀기(&["a.txt", "b.txt"]);
        let 남은 = apply_conflict_choice(풀기, &["a.txt".to_owned()], ConflictDecision::Skip)
            .expect("남는 것이 있다");
        assert_eq!(이름들(&남은), vec!["b.txt"]);
    }
}
