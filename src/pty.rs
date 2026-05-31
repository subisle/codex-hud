use crossterm::terminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherSurface {
    Inline,
    Fallback,
}

impl LauncherSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherEnvironment {
    pub surface: LauncherSurface,
    pub fallback_surface: String,
    pub layout: PtyLayout,
}

impl LauncherEnvironment {
    pub fn new(
        surface: LauncherSurface,
        fallback_surface: impl Into<String>,
        layout: PtyLayout,
    ) -> Self {
        Self {
            surface,
            fallback_surface: fallback_surface.into(),
            layout,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyLayout {
    pub total_rows: u16,
    pub main_rows: u16,
    pub bottom_rows: u16,
}

impl PtyLayout {
    pub fn is_safe(self) -> bool {
        self.total_rows == 0 || self.main_rows > 0
    }
}

pub fn reserve_bottom_rows(total_rows: u16, requested_bottom_rows: u16) -> PtyLayout {
    if total_rows == 0 {
        return PtyLayout {
            total_rows: 0,
            main_rows: 0,
            bottom_rows: 0,
        };
    }

    let bottom_rows = requested_bottom_rows.min(total_rows.saturating_sub(1));
    let main_rows = total_rows - bottom_rows;

    PtyLayout {
        total_rows,
        main_rows,
        bottom_rows,
    }
}

pub fn terminal_rows_from_env() -> Option<u16> {
    std::env::var("LINES").ok()?.parse().ok()
}

pub fn terminal_size_from_runtime_or_env() -> (u16, u16) {
    match terminal::size() {
        Ok((cols, rows)) if cols > 0 && rows > 0 => (cols, rows),
        _ => (80, terminal_rows_from_env().unwrap_or(24)),
    }
}

pub fn choose_launcher_surface(
    terminal_kind: Option<&str>,
    total_rows: u16,
    requested_bottom_rows: u16,
) -> LauncherSurface {
    if !supports_inline_terminal(terminal_kind) {
        return LauncherSurface::Fallback;
    }

    let layout = reserve_bottom_rows(total_rows, requested_bottom_rows);
    if layout.bottom_rows == 0 {
        LauncherSurface::Fallback
    } else {
        LauncherSurface::Inline
    }
}

pub fn launcher_environment(
    terminal_kind: Option<&str>,
    total_rows: u16,
    requested_bottom_rows: u16,
    configured_surface: Option<&str>,
    configured_fallback_surface: Option<&str>,
) -> LauncherEnvironment {
    let layout = reserve_bottom_rows(total_rows, requested_bottom_rows);
    let surface = match configured_surface
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) if value.eq_ignore_ascii_case("fallback") => LauncherSurface::Fallback,
        Some(value) if value.eq_ignore_ascii_case("inline-statusbar") => {
            choose_launcher_surface(terminal_kind, total_rows, requested_bottom_rows)
        }
        Some(value) if value.eq_ignore_ascii_case("inline") => {
            choose_launcher_surface(terminal_kind, total_rows, requested_bottom_rows)
        }
        _ => choose_launcher_surface(terminal_kind, total_rows, requested_bottom_rows),
    };

    let fallback_surface = configured_fallback_surface
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("split");

    LauncherEnvironment::new(surface, fallback_surface, layout)
}

pub fn launcher_env_entries(environment: &LauncherEnvironment) -> [(&'static str, String); 5] {
    [
        (
            "CODEX_HUD_LAUNCHER_SURFACE",
            environment.surface.as_str().to_string(),
        ),
        (
            "CODEX_HUD_LAYOUT_TOTAL_ROWS",
            environment.layout.total_rows.to_string(),
        ),
        (
            "CODEX_HUD_LAYOUT_MAIN_ROWS",
            environment.layout.main_rows.to_string(),
        ),
        (
            "CODEX_HUD_LAYOUT_BOTTOM_ROWS",
            environment.layout.bottom_rows.to_string(),
        ),
        (
            "CODEX_HUD_FALLBACK_SURFACE",
            environment.fallback_surface.clone(),
        ),
    ]
}

fn supports_inline_terminal(terminal_kind: Option<&str>) -> bool {
    let Some(terminal_kind) = terminal_kind.map(str::trim) else {
        return false;
    };

    if terminal_kind.is_empty() {
        return false;
    }

    !matches!(
        terminal_kind.to_ascii_lowercase().as_str(),
        "dumb" | "unknown"
    )
}
