//! 앱 공통 — 레이아웃 트리·워크스페이스·설정·자동 실행·업데이트.
//!
//! 화면은 `ui`가 그린다 — 이 모듈에는 창을 만들거나 소유하는 코드가 없다.
pub mod autostart;
pub mod bulk_rename;
pub mod compare;
pub mod drives;
pub mod favorites;
pub mod fonts;
pub mod layout;
pub mod licenses;
pub mod palette;
pub mod recent;
pub mod session_saver;
pub mod settings;
pub mod single_instance;
pub mod theme;
pub mod update;
pub mod workspace;
