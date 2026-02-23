use crate::client::{ClientApp, Panel};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

pub fn draw(f: &mut Frame, app: &mut ClientApp) {
    let size = f.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(size);

    let main_area = outer[0];
    let help_area = outer[1];

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(main_area);

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3), Constraint::Length(5)])
        .split(main_chunks[0]);

    app.layout.sinks_area = left_chunks[0];
    app.layout.volume_area = left_chunks[1];
    app.layout.audio_fx_area = left_chunks[2];

    draw_sinks_panel(f, app, left_chunks[0]);
    draw_volume_bar(f, app, left_chunks[1]);
    draw_audio_fx_panel(f, app, left_chunks[2]);
    draw_right_panel(f, app, main_chunks[1]);

    if let Some(msg) = &app.status_message {
        let help = Paragraph::new(Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Red),
        )));
        f.render_widget(help, help_area);
    } else {
        let help_text = if app.file_browser.is_some() {
            "[Up/Down] Navigate  [Enter] Open  [Backspace] Parent dir  [Esc] Close"
        } else {
            "[Tab] Switch panel  [Up/Down] Navigate  [Enter] Select  [d] Delete song  [r] Refresh sinks  [q] Quit TUI"
        };
        let help = Paragraph::new(Line::from(Span::styled(
            help_text,
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(help, help_area);
    }

    if let Some(fb) = &app.file_browser {
        draw_file_browser(f, fb, size);
    }
}

fn draw_sinks_panel(f: &mut Frame, app: &ClientApp, area: Rect) {
    let border_style = if app.focus == Panel::Sinks {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(" PipeWire Devices ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let max_width = (area.width as usize).saturating_sub(4);

    let items: Vec<ListItem> = app
        .sinks()
        .iter()
        .enumerate()
        .map(|(i, sink)| {
            let prefix = if sink.kind == "Input" { "[In] " } else { "[Out] " };
            let marker = if i == app.selected_sink() {
                " \u{2713}"
            } else {
                ""
            };
            let full = format!("{}{}{}", prefix, sink.description, marker);
            let text = truncate_with_ellipsis(&full, max_width);
            ListItem::new(text)
        })
        .collect();

    let mut state = ListState::default();
    if !app.sinks().is_empty() {
        state.select(Some(app.selected_sink()));
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut state);

    if app.focus == Panel::Sinks && !app.sinks().is_empty() {
        let sink = &app.sinks()[app.selected_sink()];
        let prefix = if sink.kind == "Input" { "[In] " } else { "[Out] " };
        let full_name = format!("{}{}", prefix, sink.description);

        if full_name.len() > max_width {
            let tooltip_y = area.y + 1 + app.selected_sink() as u16;
            if tooltip_y < area.y + area.height.saturating_sub(1) {
                let tooltip_width =
                    (full_name.len() as u16 + 2).min(f.area().width.saturating_sub(area.x));
                let tooltip_area = Rect::new(area.x, tooltip_y, tooltip_width, 1);
                f.render_widget(Clear, tooltip_area);
                let tooltip = Paragraph::new(Line::from(Span::styled(
                    format!(" {} ", full_name),
                    Style::default().fg(Color::Yellow).bg(Color::DarkGray),
                )));
                f.render_widget(tooltip, tooltip_area);
            }
        }
    }
}

fn draw_volume_bar(f: &mut Frame, app: &ClientApp, area: Rect) {
    let border_style = if app.focus == Panel::Volume {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(" Volume ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let ratio = app.volume() / 5.0;
    let filled = (ratio * inner.width as f32).round() as u16;
    let pct = (app.volume() * 100.0).round() as u16;
    let label = format!("{}%", pct);

    let bar: String = (0..inner.width)
        .map(|i| if i < filled { '\u{2588}' } else { '\u{2591}' })
        .collect();

    let label_start = inner.width.saturating_sub(label.len() as u16) / 2;
    let label_end = label_start + label.len() as u16;

    let spans: Vec<Span> = (0..inner.width)
        .map(|i| {
            let ch = if i < filled { "\u{2588}" } else { "\u{2591}" };
            let in_label = i >= label_start && i < label_end;
            if in_label {
                let label_idx = (i - label_start) as usize;
                let label_char = &label[label_idx..label_idx + 1];
                if i < filled {
                    Span::styled(label_char, Style::default().fg(Color::Black).bg(Color::Green))
                } else {
                    Span::styled(
                        label_char,
                        Style::default().fg(Color::White).bg(Color::DarkGray),
                    )
                }
            } else if i < filled {
                Span::styled(ch, Style::default().fg(Color::Green))
            } else {
                Span::styled(ch, Style::default().fg(Color::DarkGray))
            }
        })
        .collect();

    let _ = bar;
    let line = Line::from(spans);
    let paragraph = Paragraph::new(line);
    f.render_widget(paragraph, inner);
}

fn draw_audio_fx_panel(f: &mut Frame, app: &ClientApp, area: Rect) {
    let border_style = if app.focus == Panel::AudioFx {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(" Audio FX ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height < 2 {
        return;
    }

    let controls: [(&str, f32, f32, String); 2] = [
        (
            "Noise:",
            app.comfort_noise(),
            0.05,
            format!("{:.3}", app.comfort_noise()),
        ),
        (
            "EQ Mid:",
            app.eq_mid_boost(),
            3.0,
            format!("{:.1}x", app.eq_mid_boost()),
        ),
    ];

    for (idx, (label, value, max, ref value_str)) in controls.iter().enumerate() {
        let y = inner.y + idx as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let label_width = 7u16;
        let value_label_width = value_str.len() as u16 + 1;
        let bar_width = inner.width.saturating_sub(label_width + value_label_width + 1);

        let is_selected = app.focus == Panel::AudioFx && app.selected_fx == idx;

        let label_style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let label_span = Span::styled(format!("{:<7}", label), label_style);

        let ratio = if *max > 0.0 { value / max } else { 0.0 };
        let filled = (ratio * bar_width as f32).round() as u16;

        let bar_spans: Vec<Span> = (0..bar_width)
            .map(|i| {
                if i < filled {
                    Span::styled("\u{2588}", Style::default().fg(Color::Magenta))
                } else {
                    Span::styled("\u{2591}", Style::default().fg(Color::DarkGray))
                }
            })
            .collect();

        let val_span = Span::styled(format!(" {}", value_str), Style::default().fg(Color::White));

        let mut spans = vec![label_span];
        spans.extend(bar_spans);
        spans.push(val_span);

        let line = Line::from(spans);
        let row_area = Rect::new(inner.x, y, inner.width, 1);
        f.render_widget(Paragraph::new(line), row_area);
    }
}

fn truncate_with_ellipsis(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_string()
    } else if max_width <= 3 {
        s.chars().take(max_width).collect()
    } else {
        let mut truncated: String = s.chars().take(max_width - 3).collect();
        truncated.push_str("...");
        truncated
    }
}

fn draw_right_panel(f: &mut Frame, app: &mut ClientApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    app.layout.add_button_area = chunks[0];
    app.layout.songs_area = chunks[1];

    draw_add_button(f, app, chunks[0]);
    draw_songs_panel(f, app, chunks[1]);
}

fn draw_add_button(f: &mut Frame, app: &ClientApp, area: Rect) {
    let border_style = if app.focus == Panel::AddButton {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let text = if app.focus == Panel::AddButton {
        Span::styled(
            " [ + Add Songs ] ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" [ + Add Songs ] ", Style::default().fg(Color::White))
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style);

    let paragraph = Paragraph::new(Line::from(text)).block(block);
    f.render_widget(paragraph, area);
}

fn draw_songs_panel(f: &mut Frame, app: &ClientApp, area: Rect) {
    let border_style = if app.focus == Panel::Songs {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(" Songs ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let items: Vec<ListItem> = app
        .songs()
        .iter()
        .map(|song| {
            let playing = app
                .now_playing()
                .is_some_and(|np| np == song.name);
            let text = if playing {
                format!("\u{25b6} {} (playing)", song.name)
            } else {
                song.name.clone()
            };
            ListItem::new(text)
        })
        .collect();

    let mut state = ListState::default();
    if !app.songs().is_empty() {
        state.select(Some(app.selected_song()));
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut state);
}

fn draw_file_browser(f: &mut Frame, fb: &crate::filebrowser::FileBrowser, area: Rect) {
    let popup_area = centered_rect(60, 70, area);

    f.render_widget(Clear, popup_area);

    let title = format!(" {} ", fb.current_dir.display());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let items: Vec<ListItem> = fb
        .entries
        .iter()
        .map(|entry| {
            if entry.is_dir {
                ListItem::new(format!("\u{1f4c1} {}/", entry.name))
                    .style(Style::default().fg(Color::Blue))
            } else {
                ListItem::new(format!("  {}", entry.name))
            }
        })
        .collect();

    let mut state = ListState::default();
    if !fb.entries.is_empty() {
        state.select(Some(fb.selected));
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, popup_area, &mut state);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
