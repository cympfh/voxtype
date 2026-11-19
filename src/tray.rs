//! デスクトップ窓をタスクトレイへしまう／戻す判断。
//!
//! 実際のトレイアイコンは Slint の `SystemTrayIcon` (`ui/main.slint` の VoxtypeTray) が担う。
//! こちらは「最小化されたら隠す」「戻す」「閉じる」の状態遷移だけを持ち、テストできるようにしている。

/// デスクトップ窓の見え方
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DesktopState {
    /// タスクバーに出て、操作できる
    #[default]
    Shown,
    /// 窓は隠してあり、トレイのアイコンだけが見える
    InTray,
}

/// 窓またはトレイから来る出来事
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopEvent {
    /// ユーザーが窓を最小化しようとした
    Minimized,
    /// トレイのアイコンをクリックした、またはメニューの「表示」
    RestoreRequested,
    /// 閉じるボタン、トレイの「終了」、またはアプリ全体の終了
    CloseRequested,
}

/// `handle_desktop` が呼び出し側に頼む動作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopAction {
    None,
    /// 窓を hide してタスクバーからも消す
    HideToTray,
    /// 最小化を解除して窓を show する
    RestoreWindow,
    /// トレイアイコンを隠してイベントループを終える
    Quit,
}

/// 今の見え方と出来事から、次の見え方と必要な動作を返す
pub fn handle_desktop(state: DesktopState, event: DesktopEvent) -> (DesktopState, DesktopAction) {
    match event {
        DesktopEvent::Minimized => match state {
            DesktopState::Shown => (DesktopState::InTray, DesktopAction::HideToTray),
            DesktopState::InTray => (DesktopState::InTray, DesktopAction::None),
        },
        DesktopEvent::RestoreRequested => (DesktopState::Shown, DesktopAction::RestoreWindow),
        DesktopEvent::CloseRequested => (state, DesktopAction::Quit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimize_from_shown_hides_to_tray() {
        assert_eq!(
            handle_desktop(DesktopState::Shown, DesktopEvent::Minimized),
            (DesktopState::InTray, DesktopAction::HideToTray)
        );
    }

    #[test]
    fn minimize_while_already_in_tray_does_nothing() {
        // hide したあとも is_minimized が true のまま残ることがある
        assert_eq!(
            handle_desktop(DesktopState::InTray, DesktopEvent::Minimized),
            (DesktopState::InTray, DesktopAction::None)
        );
    }

    #[test]
    fn restore_from_tray_shows_window() {
        assert_eq!(
            handle_desktop(DesktopState::InTray, DesktopEvent::RestoreRequested),
            (DesktopState::Shown, DesktopAction::RestoreWindow)
        );
    }

    #[test]
    fn restore_while_shown_is_idempotent() {
        // すでに出ているときの「表示」は、show + 最小化解除をもう一度やるだけ
        assert_eq!(
            handle_desktop(DesktopState::Shown, DesktopEvent::RestoreRequested),
            (DesktopState::Shown, DesktopAction::RestoreWindow)
        );
    }

    #[test]
    fn close_from_either_state_quits() {
        assert_eq!(
            handle_desktop(DesktopState::Shown, DesktopEvent::CloseRequested),
            (DesktopState::Shown, DesktopAction::Quit)
        );
        assert_eq!(
            handle_desktop(DesktopState::InTray, DesktopEvent::CloseRequested),
            (DesktopState::InTray, DesktopAction::Quit)
        );
    }
}
