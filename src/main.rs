use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
use futures::StreamExt;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Line as CanvasLine},
        Bar, BarChart, BarGroup, Block, Borders, Clear, Paragraph,
    },
    Frame,
};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

#[cfg(not(target_os = "windows"))]
use surge_ping::{Client, Config as PingConfig, PingIdentifier, PingSequence};

#[cfg(target_os = "windows")]
use winping;

// ALVO
#[derive(Clone, Debug)]
struct TargetSpec {
    host: String,
    port: Option<u16>,
}

impl TargetSpec {
    fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.starts_with('[') {
            if let Some(end) = s.find(']') {
                let host = s[1..end].to_string();
                let port = s[end + 1..].strip_prefix(':').and_then(|p| p.parse().ok());
                return Self { host, port };
            }
        }
        if let Some(colon) = s.rfind(':') {
            let after = &s[colon + 1..];
            if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(port) = after.parse::<u16>() {
                    return Self {
                        host: s[..colon].to_string(),
                        port: Some(port),
                    };
                }
            }
        }
        Self { host: s.to_string(), port: None }
    }

    fn display(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{}", self.host, p),
            None => self.host.clone(),
        }
    }
}

// CONFIGURAÇÃO (sintaxe PsPing)
#[derive(Clone)]
struct Config {
    targets: Vec<TargetSpec>,
    interval_ms: u64,
    count: Option<u64>,
    duration_secs: Option<f64>,
    continuous: bool,
    warmup: u64,
    payload_size: usize,
    alert_rtt_ms: Option<f64>,
    alert_loss_pct: Option<f64>,
    ipv6: bool,
    report: bool,
    quiet: bool,
}

impl Config {
    fn parse() -> Result<Self, String> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut cfg = Config {
            targets: Vec::new(),
            interval_ms: 1000,
            count: Some(100),
            duration_secs: None,
            continuous: false,
            warmup: 0,
            payload_size: 56,
            alert_rtt_ms: None,
            alert_loss_pct: None,
            ipv6: false,
            report: false,
            quiet: false,
        };
        let mut positional: Vec<TargetSpec> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-m" => {
                    i += 1;
                    let list = args.get(i).ok_or("-m requer valor")?;
                    for t in list.split(',') {
                        let t = t.trim();
                        if !t.is_empty() {
                            positional.push(TargetSpec::parse(t));
                        }
                    }
                }
                "-n" => {
                    i += 1;
                    let v = args.get(i).ok_or("-n requer valor")?;
                    cfg.continuous = false;
                    if let Some(secs_str) = v.strip_suffix('s') {
                        let secs: f64 = secs_str
                            .parse()
                            .map_err(|_| format!("-n {v}: valor inválido"))?;
                        cfg.duration_secs = Some(secs);
                        cfg.count = None;
                    } else {
                        let c: u64 = v
                            .parse()
                            .map_err(|_| format!("-n {v}: valor inválido"))?;
                        cfg.count = Some(c);
                        cfg.duration_secs = None;
                    }
                }
                "-i" => {
                    i += 1;
                    let v = args.get(i).ok_or("-i requer valor")?;
                    let secs: f64 = v
                        .parse()
                        .map_err(|_| format!("-i {v}: valor inválido"))?;
                    cfg.interval_ms = if secs <= 0.0 { 10 } else { (secs * 1000.0).round() as u64 };
                }
                "-l" => {
                    i += 1;
                    let v = args.get(i).ok_or("-l requer valor")?;
                    cfg.payload_size = parse_size(v)?;
                }
                "-t" => {
                    cfg.continuous = true;
                    cfg.count = None;
                    cfg.duration_secs = None;
                }
                "-w" => {
                    i += 1;
                    let v = args.get(i).ok_or("-w requer valor")?;
                    cfg.warmup = v.parse().map_err(|_| format!("-w {v}: valor inválido"))?;
                }
                "-r" | "--report" => cfg.report = true,
                "-q" | "--quiet" => cfg.quiet = true,
                "-ipv4" => cfg.ipv6 = false,
                "-ipv6" => cfg.ipv6 = true,
                "--alert-rtt" => {
                    i += 1;
                    cfg.alert_rtt_ms = args.get(i).and_then(|s| s.parse().ok());
                }
                "--alert-loss" => {
                    i += 1;
                    cfg.alert_loss_pct = args.get(i).and_then(|s| s.parse().ok());
                }
                "-h" | "--help" => {
                    println!(
"Uso: rsping [host[:port] ...] [opções]

Alvos:
  rsping 8.8.8.8
  rsping google.com:443        (TCP ping na porta 443)
  rsping -m 8.8.8.8,1.1.1.1    (múltiplos alvos — sequencial)

Opções:
  -m <a,b,c>      Múltiplos alvos separados por vírgula
  -n <N>          Número de pings por alvo (padrão: 100)
  -n <N>s         Duração em segundos por alvo (ex: -n 10s)
  -i <segundos>   Intervalo entre pings (padrão: 1; 0 = rápido)
  -l <bytes>      Tamanho do payload (k/m para KB/MB)
  -t              Ping contínuo até a tecla q (paralelo entre alvos)
  -w <N>          Descartar os primeiros N pings (warmup)
  -r, --report    (compatibilidade — o relatório agora é impresso sempre)
  -q, --quiet     Suprime o relatório final (útil em scripts)
  -ipv4 / -ipv6   Forçar família de endereço
  --alert-rtt <ms>   Alertar se a média passar deste valor
  --alert-loss <%>   Alertar se a perda passar deste valor
  -h, --help      Esta ajuda

Teclas durante a execução:
  q / Esc   Abre o menu (Sair / Repetir / Continuar)
  Espaço    Pausar / retomar
  Tab       Trocar alvo
  ↑ / ↓     Rolar o log
  End       Voltar ao auto-scroll
  e         Exportar CSV
  Ctrl+C    Sair"
                    );
                    std::process::exit(0);
                }
                other => {
                    if other.starts_with('-') && other.len() > 1 {
                        return Err(format!("Opção desconhecida: {other}"));
                    }
                    positional.push(TargetSpec::parse(other));
                }
            }
            i += 1;
        }
        if positional.is_empty() {
            positional.push(TargetSpec::parse("8.8.8.8"));
        }
        cfg.targets = positional;
        Ok(cfg)
    }

    fn mode_label(&self) -> String {
        if self.continuous {
            "contínuo".to_string()
        } else if let Some(secs) = self.duration_secs {
            format!("{secs}s/alvo")
        } else if let Some(c) = self.count {
            format!("{c} pings/alvo")
        } else {
            "?".to_string()
        }
    }
}

fn parse_size(s: &str) -> Result<usize, String> {
    let s = s.trim().to_lowercase();
    let (num, mult) = if let Some(n) = s.strip_suffix('k') {
        (n.to_string(), 1024)
    } else if let Some(n) = s.strip_suffix('m') {
        (n.to_string(), 1024 * 1024)
    } else {
        (s.clone(), 1)
    };
    let n: usize = num.parse().map_err(|_| format!("tamanho inválido: {s}"))?;
    Ok(n * mult)
}

// MENSAGENS
#[derive(Clone)]
struct PingUpdate {
    target_idx: usize,
    rtt_us: Option<u64>,
    timestamp: Instant,
    seq: u16,
}

enum Message {
    Ping(PingUpdate),
    Done(usize),
}

// ESTATÍSTICAS
struct TargetStats {
    sent: u64,
    received: u64,
    lost: u64,
    min_us: Option<u64>,
    max_us: Option<u64>,
    sum_us: u64,
    count_valid: u64,
    last_transit: Option<f64>,
    jitter: f64,
    last_rtt_us: Option<u64>,
    spike_count: u64,
    sum_sq_us: f64,
    warmup_left: u64,
    samples_us: VecDeque<u64>,
    history: VecDeque<Option<u64>>,
    pings_done: u64,
}

impl TargetStats {
    fn new(warmup: u64, history_len: usize) -> Self {
        Self {
            sent: 0,
            received: 0,
            lost: 0,
            min_us: None,
            max_us: None,
            sum_us: 0,
            count_valid: 0,
            last_transit: None,
            jitter: 0.0,
            last_rtt_us: None,
            spike_count: 0,
            sum_sq_us: 0.0,
            warmup_left: warmup,
            samples_us: VecDeque::with_capacity(10_000),
            history: VecDeque::with_capacity(history_len),
            pings_done: 0,
        }
    }

    fn record(&mut self, rtt_us: Option<u64>, history_len: usize) {
        self.pings_done += 1;
        if self.warmup_left > 0 {
            self.warmup_left -= 1;
            return;
        }
        self.sent += 1;
        if self.history.len() == history_len {
            self.history.pop_front();
        }
        self.history.push_back(rtt_us);

        match rtt_us {
            Some(us) => {
                self.received += 1;
                self.sum_us += us;
                self.sum_sq_us += (us as f64).powi(2);
                self.count_valid += 1;
                self.min_us = Some(self.min_us.map_or(us, |m| m.min(us)));
                self.max_us = Some(self.max_us.map_or(us, |m| m.max(us)));

                let transit = us as f64 / 1000.0;
                if let Some(prev) = self.last_transit {
                    let d = (transit - prev).abs();
                    self.jitter += (d - self.jitter) / 16.0;
                }
                self.last_transit = Some(transit);

                if let Some(prev) = self.last_rtt_us {
                    if us > prev.saturating_mul(2) && us > 1_000 {
                        self.spike_count += 1;
                    }
                }
                self.last_rtt_us = Some(us);

                if self.samples_us.len() >= 10_000 {
                    self.samples_us.pop_front();
                }
                self.samples_us.push_back(us);
            }
            None => self.lost += 1,
        }
    }

    fn avg_ms(&self) -> Option<f64> {
        if self.count_valid == 0 {
            None
        } else {
            Some((self.sum_us as f64 / self.count_valid as f64) / 1000.0)
        }
    }

    fn stddev_ms(&self) -> Option<f64> {
        if self.count_valid < 2 {
            return None;
        }
        let mean = self.sum_us as f64 / self.count_valid as f64;
        let variance = (self.sum_sq_us / self.count_valid as f64) - mean.powi(2);
        Some(variance.max(0.0).sqrt() / 1000.0)
    }

    fn loss_pct(&self) -> f64 {
        if self.sent == 0 {
            0.0
        } else {
            (self.lost as f64 / self.sent as f64) * 100.0
        }
    }

    fn outlier_threshold_us(&self) -> Option<f64> {
        let avg = self.avg_ms()? * 1000.0;
        let sd = self.stddev_ms()? * 1000.0;
        Some(avg + 3.0 * sd)
    }

    fn percentiles(&self) -> (Option<f64>, Option<f64>, Option<f64>) {
        if self.samples_us.is_empty() {
            return (None, None, None);
        }
        let mut v: Vec<u64> = self.samples_us.iter().copied().collect();
        v.sort_unstable();
        let p = |q: f64| -> f64 {
            let rank = (q / 100.0 * v.len() as f64).ceil() as usize;
            let idx = rank.saturating_sub(1).min(v.len() - 1);
            v[idx] as f64 / 1000.0
        };
        (Some(p(50.0)), Some(p(95.0)), Some(p(99.0)))
    }
}

// MENU
const MENU_ITEMS: &[&str] = &["Sair", "Repetir", "Continuar"];

struct MenuState {
    selected: usize,
}

// APP
struct App {
    cfg: Config,
    names: Vec<String>,
    ips: Vec<String>,
    stats: Vec<TargetStats>,
    logs: Vec<VecDeque<String>>,
    current: usize,
    paused: bool,
    start: Instant,
    end: Option<Instant>,
    scroll: usize,
    started: Vec<bool>,
    finished: Vec<bool>,
    active: Option<usize>,
    menu: Option<MenuState>,
}

impl App {
    fn new(cfg: Config, ips: Vec<String>) -> Self {
        let n = cfg.targets.len();
        Self {
            names: cfg.targets.iter().map(|t| t.display()).collect(),
            ips,
            stats: (0..n).map(|_| TargetStats::new(cfg.warmup, 300)).collect(),
            logs: (0..n).map(|_| VecDeque::with_capacity(500)).collect(),
            cfg,
            current: 0,
            paused: false,
            start: Instant::now(),
            end: None,
            scroll: 0,
            started: vec![false; n],
            finished: vec![false; n],
            active: None,
            menu: None,
        }
    }

    fn begin_target(&mut self, idx: usize) {
        if idx < self.started.len() {
            self.started[idx] = true;
            self.active = Some(idx);
        }
    }

    fn begin_all_passive(&mut self) {
        for s in self.started.iter_mut() {
            *s = true;
        }
    }

    fn end_all(&mut self) {
        self.active = None;
        if self.end.is_none() {
            self.end = Some(Instant::now());
        }
    }

    fn push_message(&mut self, msg: Message) {
        match msg {
            Message::Ping(u) => {
                if self.menu.is_none() {
                    self.push_update(u);
                }
            }
            Message::Done(idx) => {
                if idx < self.finished.len() {
                    self.finished[idx] = true;
                }
            }
        }
    }

    fn push_update(&mut self, u: PingUpdate) {
        if self.paused {
            return;
        }
        let idx = u.target_idx;
        if idx >= self.stats.len() {
            return;
        }
        let was_warmup = self.stats[idx].warmup_left > 0;
        self.stats[idx].record(u.rtt_us, 300);
        if was_warmup {
            return;
        }
        let elapsed = u.timestamp.duration_since(self.start);
        let msg = match u.rtt_us {
            Some(us) => format!(
                "[{:>7.1}s] seq={:<5} {:>7.2} ms",
                elapsed.as_secs_f64(),
                u.seq,
                us as f64 / 1000.0
            ),
            None => format!(
                "[{:>7.1}s] seq={:<5} Timeout",
                elapsed.as_secs_f64(),
                u.seq
            ),
        };
        if self.logs[idx].len() >= 500 {
            self.logs[idx].pop_front();
        }
        self.logs[idx].push_back(msg);
    }

    fn all_done(&self) -> bool {
        if self.cfg.continuous {
            return false;
        }
        self.finished.iter().all(|&b| b)
    }

    fn view(&self, frame: &mut Frame) {
        let area = frame.area();

        let show_tabs = self.names.len() > 1;
        let tab_h: u16 = if show_tabs { 3 } else { 0 };

        let outer = Layout::vertical([
            Constraint::Length(tab_h),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(area);

        if show_tabs {
            self.render_tab_bar(frame, outer[0]);
        }

        let root = Layout::horizontal([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)])
            .split(outer[1]);
        self.render_left(frame, root[0]);

        let right = Layout::vertical([
            Constraint::Length(8),
            Constraint::Length(4),
            Constraint::Min(10),
        ])
        .split(root[1]);

        self.render_stats(frame, right[0]);
        self.render_histogram(frame, right[1]);
        self.render_braille(frame, right[2]);

        self.render_footer(frame, outer[2]);
        self.render_menu(frame, area);
    }

    fn render_tab_bar(&self, frame: &mut Frame, area: Rect) {
        let mut spans: Vec<Span> = Vec::new();
        for (i, name) in self.names.iter().enumerate() {
            let is_current = i == self.current;
            let is_finished = self.finished[i];
            let is_active = self.active == Some(i);
            let is_started = self.started[i];

            let content = if is_finished {
                let avg = self.stats[i].avg_ms().unwrap_or(0.0);
                let loss = self.stats[i].loss_pct();
                if loss > 0.0 {
                    format!(" ✓ {} {:.0}ms {:.0}%↓ ", name, avg, loss)
                } else {
                    format!(" ✓ {} {:.0}ms ", name, avg)
                }
            } else if is_active {
                format!(" ● {} rodando ", name)
            } else if is_started {
                format!(" ● {} … ", name)
            } else {
                format!(" ○ {} aguardando ", name)
            };

            let style = if is_current {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if is_finished {
                Style::default().fg(Color::Green)
            } else if is_active {
                Style::default().fg(Color::Yellow)
            } else if is_started {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            spans.push(Span::styled(content, style));
            if i < self.names.len() - 1 {
                spans.push(Span::raw(" "));
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("Alvos ({} total — Tab alterna)", self.names.len())),
            ),
            area,
        );
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let hint = |k: &'static str, d: &'static str| -> Vec<Span<'static>> {
            vec![
                Span::styled(
                    format!(" {k} "),
                    Style::default().fg(Color::Black).bg(Color::DarkGray),
                ),
                Span::styled(format!(" {d} "), Style::default().fg(Color::Gray)),
            ]
        };

        let mut spans: Vec<Span> = Vec::new();
        spans.extend(hint("Espaço", "pausa"));
        spans.extend(hint("Tab", "alvo"));
        spans.extend(hint("q", "menu"));
        spans.extend(hint("e", "CSV"));
        spans.extend(hint("↑↓", "scroll"));
        spans.extend(hint("End", "auto"));

        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_menu(&self, frame: &mut Frame, area: Rect) {
        let menu = match &self.menu {
            Some(m) => m,
            None => return,
        };
        let width: u16 = 26;
        let height: u16 = (MENU_ITEMS.len() as u16) + 4;
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let menu_area = Rect {
            x,
            y,
            width: width.min(area.width),
            height: height.min(area.height),
        };

        let items: Vec<Line> = MENU_ITEMS
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let style = if i == menu.selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(Span::styled(format!("  {:<12}", item), style))
            })
            .collect();

        frame.render_widget(Clear, menu_area);
        frame.render_widget(
            Paragraph::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Menu (↑/↓ + Enter) ")
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            menu_area,
        );
    }

    fn render_left(&self, frame: &mut Frame, area: Rect) {
        let idx = self.current;
        let st = &self.stats[idx];

        let (status_bg, status_text) = if self.menu.is_some() {
            (Color::Magenta, " MENU ")
        } else if self.paused {
            (Color::Yellow, " PAUSADO ")
        } else if self.finished[idx] {
            (Color::Blue, " CONCLUÍDO ")
        } else if !self.started[idx] {
            (Color::DarkGray, " AGUARDANDO ")
        } else {
            (Color::Green, " EM ANDAMENTO ")
        };
        let status = Span::styled(status_text, Style::default().bg(status_bg).fg(Color::Black));

        let header = vec![
            Line::from(vec![
                status,
                Span::raw(" "),
                Span::styled(
                    format!(
                        "Pinging {} [{}]",
                        self.names[idx],
                        self.ips.get(idx).map(|s| s.as_str()).unwrap_or("?")
                    ),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ({})", self.cfg.mode_label()),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            Line::from(""),
            Line::from(format!(
                "Sent = {}, Received = {}, Lost = {} ({:.0}% loss)",
                st.sent, st.received, st.lost, st.loss_pct()
            )),
            Line::from(format!(
                "Minimum = {}ms, Maximum = {}ms, Average = {:.2}ms",
                fmt_ms(st.min_us),
                fmt_ms(st.max_us),
                st.avg_ms().unwrap_or(0.0)
            )),
        ];

        let header_h = 6u16;
        frame.render_widget(
            Paragraph::new(header)
                .block(Block::default().borders(Borders::ALL).title("PsPing Output")),
            Rect { x: area.x, y: area.y, width: area.width, height: header_h },
        );

        let log_area = Rect {
            x: area.x,
            y: area.y + header_h,
            width: area.width,
            height: area.height.saturating_sub(header_h),
        };

        let inner_height = log_area.height.saturating_sub(2) as usize;
        let total = self.logs[idx].len();
        let visible_count = inner_height.min(total);
        let end = total.saturating_sub(self.scroll);
        let start = end.saturating_sub(visible_count);
        let auto_tag = if self.scroll == 0 { " [AUTO]" } else { "" };

        let visible: Vec<Line> = self.logs[idx]
            .iter()
            .skip(start)
            .take(visible_count)
            .map(|l| {
                let color = if l.contains("Timeout") { Color::Red } else { Color::Green };
                Line::from(Span::styled(l.as_str(), Style::default().fg(color)))
            })
            .collect();

        frame.render_widget(
            Paragraph::new(visible).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("Replies (↑/↓ scroll, End=auto){}", auto_tag)),
            ),
            log_area,
        );
    }

    fn render_stats(&self, frame: &mut Frame, area: Rect) {
        let idx = self.current;
        let st = &self.stats[idx];
        let (p50, p95, p99) = st.percentiles();
        let sd = st.stddev_ms();
        let outlier = st.outlier_threshold_us();
        let is_tcp = self.cfg.targets[idx].port.is_some();

        let alert = self
            .cfg
            .alert_rtt_ms
            .map_or(false, |t| st.avg_ms().map_or(false, |a| a > t))
            || self
                .cfg
                .alert_loss_pct
                .map_or(false, |t| st.loss_pct() > t);
        let border_color = if alert { Color::Red } else { Color::White };

        let lines = vec![
            Line::from(vec![
                Span::styled("Alvo:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} ({} de {})", self.names[idx], idx + 1, self.names.len()),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
                Span::styled("Proto: ", Style::default().fg(Color::DarkGray)),
                Span::raw(if is_tcp { "TCP" } else { "ICMP" }),
                Span::raw("   "),
                Span::styled("Intervalo: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{}ms", self.cfg.interval_ms)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Média: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    st.avg_ms().map_or("—".into(), |v| format!("{v:.2} ms")),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw("   "),
                Span::styled("Desvio: ", Style::default().fg(Color::DarkGray)),
                Span::raw(sd.map_or("—".into(), |v| format!("{v:.2} ms"))),
                Span::raw("   "),
                Span::styled("Jitter (RFC): ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.2} ms", st.jitter),
                    Style::default().fg(Color::Magenta),
                ),
            ]),
            Line::from(vec![
                Span::styled("P50: ", Style::default().fg(Color::DarkGray)),
                Span::raw(p50.map_or("—".into(), |v| format!("{v:.2} ms"))),
                Span::raw("   "),
                Span::styled("P95: ", Style::default().fg(Color::DarkGray)),
                Span::raw(p95.map_or("—".into(), |v| format!("{v:.2} ms"))),
                Span::raw("   "),
                Span::styled("P99: ", Style::default().fg(Color::DarkGray)),
                Span::raw(p99.map_or("—".into(), |v| format!("{v:.2} ms"))),
            ]),
            Line::from(vec![
                Span::styled("Perda: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.1}%", st.loss_pct()),
                    Style::default().fg(if st.loss_pct() > 0.0 { Color::Red } else { Color::Green }),
                ),
                Span::raw("   "),
                Span::styled("Spikes: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{}", st.spike_count)),
                Span::raw("   "),
                Span::styled("Limiar outlier: ", Style::default().fg(Color::DarkGray)),
                Span::raw(outlier.map_or("—".into(), |v| format!("{:.1} ms", v / 1000.0))),
            ]),
        ];

        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Conexão & Estatísticas")
                    .border_style(Style::default().fg(border_color)),
            ),
            area,
        );
    }

    fn render_histogram(&self, frame: &mut Frame, area: Rect) {
        let st = &self.stats[self.current];
        let (min_ms, max_ms) = match (st.min_us, st.max_us) {
            (Some(a), Some(b)) if b > a => (a as f64 / 1000.0, b as f64 / 1000.0),
            _ => (0.0, 100.0),
        };
        let n_buckets = 10usize;
        let width = (max_ms - min_ms) / n_buckets as f64;
        let mut buckets = vec![0u64; n_buckets];
        for &us in &st.samples_us {
            let ms = us as f64 / 1000.0;
            let idx = if width > 0.0 {
                ((ms - min_ms) / width).floor() as usize
            } else {
                0
            };
            let idx = idx.min(n_buckets - 1);
            buckets[idx] += 1;
        }

        let bars: Vec<Bar> = buckets
            .iter()
            .enumerate()
            .map(|(i, &count)| {
                let label = format!("{:.0}", min_ms + (i as f64 + 0.5) * width);
                Bar::default()
                    .value(count)
                    .label(Line::from(label))
                    .style(Style::default().fg(Color::Cyan))
            })
            .collect();

        let chart = BarChart::default()
            .data(BarGroup::default().bars(&bars))
            .bar_width(3)
            .bar_gap(1)
            .block(Block::default().borders(Borders::ALL).title("Histograma (ms → contagem)"));
        frame.render_widget(chart, area);
    }

    fn render_braille(&self, frame: &mut Frame, area: Rect) {
        let st = &self.stats[self.current];
        let data: Vec<Option<u64>> = st.history.iter().copied().collect();
        if data.iter().all(|v| v.is_none()) {
            frame.render_widget(
                Paragraph::new("Sem dados ainda...")
                    .block(Block::default().borders(Borders::ALL).title("Latência (Braille)")),
                area,
            );
            return;
        }

        let max_us = data.iter().filter_map(|&v| v).max().unwrap_or(100_000);
        let max_ms = (max_us as f64 / 1000.0).max(10.0);
        let n = data.len().max(1);
        let threshold = st.outlier_threshold_us();

        let canvas = Canvas::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("Latência Braille — máx: {:.1} ms", max_ms)),
            )
            .marker(symbols::Marker::Braille)
            .x_bounds([0.0, n as f64])
            .y_bounds([0.0, max_ms * 1.1])
            .paint(|ctx| {
                ctx.draw(&CanvasLine {
                    x1: 0.0,
                    y1: 0.0,
                    x2: n as f64,
                    y2: 0.0,
                    color: Color::DarkGray,
                });
                let mut prev: Option<(f64, f64)> = None;
                for (i, item) in data.iter().enumerate() {
                    if let Some(us) = item {
                        let x = i as f64;
                        let y = (*us as f64) / 1000.0;
                        let color = if let Some(t) = threshold {
                            if (*us as f64) > t { Color::Red } else { Color::Green }
                        } else {
                            Color::Green
                        };
                        if let Some((px, py)) = prev {
                            ctx.draw(&CanvasLine { x1: px, y1: py, x2: x, y2: y, color });
                        }
                        ctx.print(x, y, Span::styled("•", Style::default().fg(color)));
                        prev = Some((x, y));
                    } else {
                        prev = None;
                    }
                }
            });
        frame.render_widget(canvas, area);
    }
}

fn fmt_ms(v: Option<u64>) -> String {
    v.map_or("—".into(), |us| format!("{:.2}", us as f64 / 1000.0))
}

// COLETORES
fn spawn_target(
    idx: usize,
    spec: &TargetSpec,
    cfg: &Config,
    tx: mpsc::UnboundedSender<Message>,
) {
    let spec = spec.clone();
    let c = cfg.clone();
    if let Some(port) = spec.port {
        tokio::spawn(async move {
            tcp_loop(idx, spec.host, port, c, tx).await;
        });
    } else {
        tokio::spawn(async move {
            icmp_loop(idx, spec.host, c, tx).await;
        });
    }
}

// => ICMP: Linux/macOS (surge-ping, assíncrono)
#[cfg(not(target_os = "windows"))]
async fn icmp_loop(
    target_idx: usize,
    target: String,
    cfg: Config,
    tx: mpsc::UnboundedSender<Message>,
) {
    let payload = vec![0u8; cfg.payload_size];
    let mut seq = 0u16;
    let mut sent: u64 = 0;
    let mut client: Option<Client> = None;
    let interval = Duration::from_millis(cfg.interval_ms);
    let start_time = Instant::now();
    let deadline = cfg
        .duration_secs
        .map(|d| start_time + Duration::from_secs_f64(d));
    let total_pings = if cfg.continuous {
        None
    } else {
        cfg.count.map(|c| c + cfg.warmup)
    };
    let mut next_deadline = start_time;

    loop {
        if let Some(total) = total_pings {
            if sent >= total {
                break;
            }
        }
        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                break;
            }
        }

        if client.is_none() {
            match Client::new(&PingConfig::default()) {
                Ok(c) => client = Some(c),
                Err(e) => {
                    eprintln!("[{target}] falha ao criar cliente ICMP: {e}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            }
        }

        let addr: IpAddr = match resolve(&target, cfg.ipv6).await {
            Ok(a) => a,
            Err(e) => {
                eprintln!("[{target}] DNS: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        let pinger = client.as_mut().unwrap();
        let mut p = pinger
            .pinger(addr, PingIdentifier(0x1234 + target_idx as u16))
            .await;
        p.timeout(interval.max(Duration::from_millis(500)));

        let rtt_us = match p.ping(PingSequence(seq), &payload).await {
            Ok((_pkt, dur)) => Some(dur.as_micros() as u64),
            Err(_) => None,
        };

        if tx
            .send(Message::Ping(PingUpdate {
                target_idx,
                rtt_us,
                timestamp: Instant::now(),
                seq,
            }))
            .is_err()
        {
            return;
        }

        seq = seq.wrapping_add(1);
        sent += 1;

        next_deadline += interval;
        let now = Instant::now();
        if next_deadline <= now {
            next_deadline = now;
        }
        let remaining = next_deadline.saturating_duration_since(now);
        let remaining = if let Some(dl) = deadline {
            remaining.min(dl.saturating_duration_since(now))
        } else {
            remaining
        };
        if !remaining.is_zero() {
            tokio::time::sleep(remaining).await;
        }
    }
    let _ = tx.send(Message::Done(target_idx));
}

// => ICMP: Ruindows Ksksks (winping-rs / IcmpSendEcho, sem admin) 
#[cfg(target_os = "windows")]
async fn icmp_loop(
    target_idx: usize,
    target: String,
    cfg: Config,
    tx: mpsc::UnboundedSender<Message>,
) {
    tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;

        let dst: IpAddr = match target.parse() {
            Ok(ip) => ip,
            Err(_) => match (target.as_str(), 0).to_socket_addrs() {
                Ok(mut addrs) => match addrs.next() {
                    Some(sa) => sa.ip(),
                    None => {
                        eprintln!("[{target}] DNS: sem endereço");
                        return;
                    }
                },
                Err(e) => {
                    eprintln!("[{target}] DNS: {e}");
                    return;
                }
            },
        };

        let pinger = match winping::Pinger::new() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[{target}] falha ao criar Pinger: {e}");
                return;
            }
        };
        let mut buffer = winping::Buffer::new();

        let interval = Duration::from_millis(cfg.interval_ms);
        let start_time = Instant::now();
        let deadline = cfg
            .duration_secs
            .map(|d| start_time + Duration::from_secs_f64(d));
        let total_pings = if cfg.continuous {
            None
        } else {
            cfg.count.map(|c| c + cfg.warmup)
        };

        let mut sent: u64 = 0;
        let mut seq: u16 = 0;

        loop {
            if let Some(total) = total_pings {
                if sent >= total {
                    break;
                }
            }
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    break;
                }
            }

            let ping_start = Instant::now();
            let rtt_us = match pinger.send(dst, &mut buffer) {
                Ok(rtt_ms) => Some((rtt_ms as u64) * 1000),
                Err(_) => None,
            };

            let update = PingUpdate {
                target_idx,
                rtt_us,
                timestamp: Instant::now(),
                seq,
            };

            if tx.send(Message::Ping(update)).is_err() {
                return;
            }

            seq = seq.wrapping_add(1);
            sent += 1;

            let elapsed = ping_start.elapsed();
            let sleep_time = interval.saturating_sub(elapsed);
            let sleep_time = if let Some(dl) = deadline {
                sleep_time.min(dl.saturating_duration_since(Instant::now()))
            } else {
                sleep_time
            };
            if !sleep_time.is_zero() {
                std::thread::sleep(sleep_time);
            }
        }

        let _ = tx.send(Message::Done(target_idx));
    })
    .await
    .ok();
}

// ── TCP (geral)
async fn tcp_loop(
    target_idx: usize,
    target: String,
    port: u16,
    cfg: Config,
    tx: mpsc::UnboundedSender<Message>,
) {
    let mut seq = 0u16;
    let mut sent: u64 = 0;
    let interval = Duration::from_millis(cfg.interval_ms);
    let start_time = Instant::now();
    let deadline = cfg
        .duration_secs
        .map(|d| start_time + Duration::from_secs_f64(d));
    let total_pings = if cfg.continuous {
        None
    } else {
        cfg.count.map(|c| c + cfg.warmup)
    };
    let mut next_deadline = start_time;

    loop {
        if let Some(total) = total_pings {
            if sent >= total {
                break;
            }
        }
        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                break;
            }
        }

        let start = Instant::now();
        let rtt_us = match resolve_socket(&target, port, cfg.ipv6).await {
            Ok(sockaddr) => match tokio::time::timeout(
                interval.max(Duration::from_millis(500)),
                TcpStream::connect(sockaddr),
            )
            .await
            {
                Ok(Ok(_stream)) => Some(start.elapsed().as_micros() as u64),
                _ => None,
            },
            Err(_) => None,
        };

        if tx
            .send(Message::Ping(PingUpdate {
                target_idx,
                rtt_us,
                timestamp: Instant::now(),
                seq,
            }))
            .is_err()
        {
            return;
        }

        seq = seq.wrapping_add(1);
        sent += 1;

        next_deadline += interval;
        let now = Instant::now();
        if next_deadline <= now {
            next_deadline = now;
        }
        let remaining = next_deadline.saturating_duration_since(now);
        let remaining = if let Some(dl) = deadline {
            remaining.min(dl.saturating_duration_since(now))
        } else {
            remaining
        };
        if !remaining.is_zero() {
            tokio::time::sleep(remaining).await;
        }
    }
    let _ = tx.send(Message::Done(target_idx));
}

async fn resolve(host: &str, ipv6: bool) -> Result<IpAddr, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip);
    }
    let mut addrs = tokio::net::lookup_host((host, 0)).await?;
    if ipv6 {
        addrs
            .find(|sa| sa.is_ipv6())
            .map(|sa| sa.ip())
            .ok_or_else(|| "sem endereço IPv6".into())
    } else {
        addrs
            .find(|sa| sa.is_ipv4())
            .map(|sa| sa.ip())
            .ok_or_else(|| "sem endereço IPv4".into())
    }
}

async fn resolve_socket(
    host: &str,
    port: u16,
    ipv6: bool,
) -> Result<std::net::SocketAddr, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(std::net::SocketAddr::new(ip, port));
    }
    let mut addrs = tokio::net::lookup_host((host, port)).await?;
    if ipv6 {
        addrs
            .find(|sa| sa.is_ipv6())
            .ok_or_else(|| "sem endereço IPv6".into())
    } else {
        addrs
            .find(|sa| sa.is_ipv4())
            .ok_or_else(|| "sem endereço IPv4".into())
    }
}
// EXPORT CSV, depois fazer em txt mesmo
fn export_csv(app: &App) -> Result<(), std::io::Error> {
    for (idx, name) in app.names.iter().enumerate() {
        let st = &app.stats[idx];
        let filename = format!("rsping_{}.csv", sanitize(name));
        let mut out = String::from("timestamp,rtt_ms,seq,status\n");
        for (i, item) in st.history.iter().enumerate() {
            let (rtt, status) = match item {
                Some(us) => (format!("{:.3}", *us as f64 / 1000.0), "ok"),
                None => ("".into(), "timeout"),
            };
            out.push_str(&format!("{},{},{},{}\n", i, rtt, i, status));
        }
        std::fs::write(&filename, out)?;
        eprintln!("CSV salvo: {filename}");
    }
    Ok(())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect()
}

// AVALIAÇÃO QUALITATIVA
fn evaluate(st: &TargetStats) -> &'static str {
    if st.count_valid < 3 {
        return "poucos dados para avaliar";
    }
    let loss = st.loss_pct();
    let avg = st.avg_ms().unwrap_or(0.0);
    let jitter = st.jitter;

    if loss > 5.0 {
        "RUIM — perda de pacotes alta"
    } else if loss > 1.0 {
        "INSTÁVEL — há perda de pacotes"
    } else if jitter > 30.0 {
        "INSTÁVEL — jitter alto"
    } else if avg > 200.0 {
        "RUIM — latência alta"
    } else if avg < 50.0 && jitter < 10.0 {
        "EXCELENTE — latência baixa, sem perda, jitter baixo"
    } else if avg < 100.0 && jitter < 20.0 {
        "BOM — desempenho adequado"
    } else {
        "RAZOÁVEL — latência ou jitter moderados"
    }
}

fn evaluate_score(st: &TargetStats) -> u8 {
    if st.count_valid < 3 {
        return 50;
    }
    let loss = st.loss_pct();
    let avg = st.avg_ms().unwrap_or(0.0);
    let jitter = st.jitter;

    let mut score: i32 = 100;
    score -= (loss * 10.0) as i32;
    score -= (jitter * 0.5) as i32;
    if avg > 50.0 {
        score -= ((avg - 50.0) / 10.0) as i32;
    }
    score.clamp(0, 100) as u8
}

fn evaluate_label(score: u8) -> &'static str {
    match score {
        90..=100 => "EXCELENTE",
        70..=89 => "BOM",
        50..=69 => "RAZOÁVEL",
        30..=49 => "INSTÁVEL",
        _ => "RUIM",
    }
}

fn progress_bar(score: u8, width: usize) -> String {
    let filled = ((score as usize * width) / 100).min(width);
    let empty = width - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

// RESUMO FINAL

fn print_summary(app: &App) {
    let sep_wide = "═".repeat(76);
    let sep_thin = "─".repeat(74);

    println!();
    println!("{}", sep_wide);
    println!("  📊  rsping — RELATÓRIO DE TESTE");
    println!("{}", sep_wide);

    let total_dur = app
        .end
        .map(|e| e.duration_since(app.start).as_secs_f64())
        .unwrap_or_else(|| app.start.elapsed().as_secs_f64());
    let testados = app.stats.iter().filter(|s| s.sent > 0).count();

    println!(
        "  Duração total: {:.2}s   ·   Modo: {}   ·   Intervalo: {}ms   ·   Warmup: {}",
        total_dur,
        app.cfg.mode_label(),
        app.cfg.interval_ms,
        app.cfg.warmup
    );
    println!(
        "  Alvos testados: {} de {}",
        testados,
        app.names.len()
    );
    println!();

    if testados == 0 {
        println!("  Nenhum dado coletado — o programa foi encerrado antes do primeiro ping.");
        println!();
        println!("{}", sep_wide);
        return;
    }

    // ── Tabela comparativa no Multi Target
    if app.names.len() > 1 {
        println!("  ┌─ Resumo comparativo {}", "─".repeat(51));
        println!(
            "  │ #  {:<30} {:>4} {:>4} {:>6} {:>7} {:>7} {:>7}",
            "Alvo", "Env", "Rec", "Perda", "Mín", "Méd", "Máx"
        );
        println!("  ├{}", sep_thin);
        for (idx, name) in app.names.iter().enumerate() {
            let st = &app.stats[idx];
            let name_trim = if name.len() > 30 { &name[..30] } else { name.as_str() };
            let label = format!(
                "{} {}",
                name_trim,
                if app.cfg.targets[idx].port.is_some() { "·TCP" } else { "·ICMP" }
            );
            if st.sent == 0 {
                println!(
                    "  │ {:<2} {:<30} {:>4} {:>4} {:>6} {:>7} {:>7} {:>7}",
                    idx + 1, label, "—", "—", "—", "—", "—", "—"
                );
            } else {
                println!(
                    "  │ {:<2} {:<30} {:>4} {:>4} {:>5.1}% {:>7.2} {:>7.2} {:>7.2}",
                    idx + 1,
                    label,
                    st.sent,
                    st.received,
                    st.loss_pct(),
                    st.min_us.unwrap_or(0) as f64 / 1000.0,
                    st.avg_ms().unwrap_or(0.0),
                    st.max_us.unwrap_or(0) as f64 / 1000.0,
                );
            }
        }
        println!("  └{}", sep_thin);
        println!();
    }

    // Detalhes por alvo 
    for (idx, name) in app.names.iter().enumerate() {
        let st = &app.stats[idx];
        let ip = app.ips.get(idx).map(|s| s.as_str()).unwrap_or("?");
        let proto = if app.cfg.targets[idx].port.is_some() { "TCP" } else { "ICMP" };

        println!(
            "  ┌─ Alvo {} de {}: {} [{}] ({}) {}",
            idx + 1,
            app.names.len(),
            name,
            ip,
            proto,
            "─".repeat(30)
        );

        if st.sent == 0 {
            println!("  │ NÃO TESTADO (o programa encerrou antes deste alvo começar)");
            println!("  └{}", sep_thin);
            println!();
            continue;
        }

        println!("  │ PACOTES");
        println!(
            "  │   Enviados ......... {:>6}      Recebidos ........ {:>6}",
            st.sent, st.received
        );
        println!(
            "  │   Perdidos ......... {:>6}      Perda ............ {:>5.1}%",
            st.lost,
            st.loss_pct()
        );

        if st.count_valid > 0 {
            println!("  │ LATÊNCIA (ms)");
            println!(
                "  │   Mínimo ........... {:>10.2}      Máximo ........... {:>10.2}",
                st.min_us.unwrap_or(0) as f64 / 1000.0,
                st.max_us.unwrap_or(0) as f64 / 1000.0
            );
            println!(
                "  │   Média ............ {:>10.2}      Desvio ........... {:>10}",
                st.avg_ms().unwrap_or(0.0),
                st.stddev_ms().map_or("—".into(), |v| format!("{v:.2}"))
            );

            let (p50, p95, p99) = st.percentiles();
            println!("  │ DISTRIBUIÇÃO (ms)");
            println!(
                "  │   P50 .............. {:>10}      P95 .............. {:>10}",
                p50.map_or("—".into(), |v| format!("{v:.2}")),
                p95.map_or("—".into(), |v| format!("{v:.2}"))
            );
            println!(
                "  │   P99 .............. {:>10}      Spikes ........... {:>10}",
                p99.map_or("—".into(), |v| format!("{v:.2}")),
                st.spike_count
            );
            println!("  │ JITTER (RFC 3550)");
            println!("  │   {:.2} ms", st.jitter);

            let score = evaluate_score(st);
            println!("  │ ESTABILIDADE");
            println!(
                "  │   {} [{}] {}",
                progress_bar(score, 40),
                score,
                evaluate_label(score)
            );
            println!("  │ Avaliação: {}", evaluate(st));
        }

        println!("  └{}", sep_thin);
        println!();
    }

    // Ranking 
    if app.names.len() > 1 {
        let mut candidatos: Vec<(usize, f64, f64, f64)> = Vec::new();
        for (idx, _) in app.names.iter().enumerate() {
            let st = &app.stats[idx];
            if let Some(avg) = st.avg_ms() {
                candidatos.push((idx, avg, st.jitter, st.loss_pct()));
            }
        }
        if candidatos.len() >= 2 {
            println!("  ┌─ Ranking {}", "─".repeat(62));
            let best_avg = candidatos
                .iter()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .unwrap();
            let best_jit = candidatos
                .iter()
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
                .unwrap();
            let worst_loss = candidatos
                .iter()
                .max_by(|a, b| a.3.partial_cmp(&b.3).unwrap())
                .unwrap();

            println!(
                "  │ Melhor latência .... {} ({:.2} ms)",
                app.names[best_avg.0], best_avg.1
            );
            println!(
                "  │ Melhor jitter ...... {} ({:.2} ms)",
                app.names[best_jit.0], best_jit.2
            );
            if worst_loss.3 > 0.0 {
                println!(
                    "  │ Maior perda ........ {} ({:.1}%)",
                    app.names[worst_loss.0], worst_loss.3
                );
            } else {
                println!("  │ Maior perda ........ nenhuma perda em todos os alvos");
            }
            println!("  └{}", sep_thin);
            println!();
        }
    }

    println!("{}", sep_wide);
    println!();
}

// MAIN
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::parse()?;

    let mut terminal = ratatui::init();

    let final_app: App = 'run: loop {
        let mut ips = Vec::new();
        for spec in &cfg.targets {
            let ip = if let Some(port) = spec.port {
                match resolve_socket(&spec.host, port, cfg.ipv6).await {
                    Ok(sa) => sa.ip().to_string(),
                    Err(_) => "?".into(),
                }
            } else {
                match resolve(&spec.host, cfg.ipv6).await {
                    Ok(ip) => ip.to_string(),
                    Err(_) => "?".into(),
                }
            };
            ips.push(ip);
        }

        let mut app = App::new(cfg.clone(), ips);
        let mut events = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

        if cfg.continuous {
            for idx in 0..cfg.targets.len() {
                spawn_target(idx, &cfg.targets[idx], &cfg, tx.clone());
            }
            app.begin_all_passive();
        } else {
            spawn_target(0, &cfg.targets[0], &cfg, tx.clone());
            app.begin_target(0);
        }

        let mut repeat = false;

        loop {
            terminal.draw(|f| app.view(f))?;

            if app.all_done() && app.menu.is_none() {
                break;
            }

            tokio::select! {
                Some(Ok(event)) = events.next() => {
                    if let Event::Key(key) = event {
                        if app.menu.is_some() {
                            let action = match key.code {
                                KeyCode::Up => {
                                    if let Some(ref mut m) = app.menu {
                                        m.selected = m.selected.saturating_sub(1);
                                    }
                                    None
                                }
                                KeyCode::Down => {
                                    if let Some(ref mut m) = app.menu {
                                        m.selected = (m.selected + 1).min(MENU_ITEMS.len() - 1);
                                    }
                                    None
                                }
                                KeyCode::Enter => app.menu.take().map(|m| m.selected),
                                KeyCode::Esc => {
                                    app.menu = None;
                                    app.paused = false;
                                    None
                                }
                                _ => None,
                            };
                            match action {
                                Some(0) => break,
                                Some(1) => { repeat = true; break; }
                                Some(2) => { app.paused = false; }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('q') | KeyCode::Esc => {
                                    app.paused = true;
                                    app.menu = Some(MenuState { selected: 0 });
                                }
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    break;
                                }
                                KeyCode::Char(' ') => app.paused = !app.paused,
                                KeyCode::Tab => {
                                    if !app.names.is_empty() {
                                        app.current = (app.current + 1) % app.names.len();
                                        app.scroll = 0;
                                    }
                                }
                                KeyCode::Up => {
                                    let total = app.logs[app.current].len();
                                    if app.scroll + 1 < total { app.scroll += 1; }
                                }
                                KeyCode::Down => {
                                    if app.scroll > 0 { app.scroll -= 1; }
                                }
                                KeyCode::End => { app.scroll = 0; }
                                KeyCode::Char('e') => { export_csv(&app).ok(); }
                                _ => {}
                            }
                        }
                    }
                }
                Some(msg) = rx.recv() => {
                    let done_idx = match &msg {
                        Message::Done(i) => Some(*i),
                        _ => None,
                    };
                    app.push_message(msg);
                    if let Some(idx) = done_idx {
                        if !cfg.continuous && app.active == Some(idx) {
                            let next = idx + 1;
                            if next < cfg.targets.len() {
                                spawn_target(next, &cfg.targets[next], &cfg, tx.clone());
                                app.begin_target(next);
                                app.current = next;
                                app.scroll = 0;
                            } else {
                                app.end_all();
                            }
                        }
                    }
                }
                _ = tick.tick() => {}
            }
        }

        if app.end.is_none() {
            app.end_all();
        }

        if !repeat {
            break 'run app;
        }
    };

    ratatui::restore();

    if !cfg.quiet {
        print_summary(&final_app);
    }

    Ok(())
}