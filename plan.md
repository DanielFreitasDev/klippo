# Klippo — clone do KDE Klipper para Ubuntu/Linux (Rust)

## Contexto

Você quer recriar o **Klipper** (gerenciador de histórico da área de transferência do KDE) como um app
para Ubuntu e Linux em geral, funcionando em **Wayland e X11**, com visual quase idêntico ao original,
busca em tempo real, atalho **Super+V**, fonte **JetBrains Mono**, modo claro/escuro, lista em ordem
decrescente, remoção item a item e limite de itens configurável (padrão 25).

A pesquisa revelou a restrição que define todo o projeto: **no GNOME Wayland (sua sessão atual — Ubuntu
26.04, GNOME 50.1) nenhum app externo consegue monitorar a área de transferência**, porque o Mutter se
recusa, por privacidade, a implementar os protocolos `wlr-data-control`/`ext-data-control`. Só código
rodando *dentro* do GNOME Shell consegue capturar (é assim que GPaste/Pano/Clipboard Indicator operam).
Em KDE Plasma 6.4+, wlroots (Sway/Hyprland) e X11, um app externo monitora normalmente — o GNOME é a exceção,
e é o alvo principal. Além disso, no GNOME Wayland não há portal de atalhos globais (usa-se *custom keybinding*
via `gsettings`) e um cliente não pode posicionar a própria janela no cursor.

**Decisões tomadas com você:**
- **Arquitetura híbrida:** núcleo + UI em **Rust**; captura direta em X11/KDE/wlroots; no GNOME Wayland uma
  pequena **extensão do GNOME Shell (GJS)** captura as cópias e envia ao daemon Rust via **D-Bus**.
- **Nome:** **Klippo** (binário/comando `klippo`).
- **Escopo:** **clone completo já no v1**, incluindo Actions por regex, QR code, sincronização
  seleção×clipboard e opção de ignorar imagens.

Resultado pretendido: um app que se comporta como o Klipper, roda de verdade no seu Ubuntu GNOME Wayland e
também em KDE/wlroots/X11, mantendo o núcleo em Rust.

---

## Stack e crates (com justificativa)

| Área | Crate(s) | Por quê |
|---|---|---|
| Runtime async / D-Bus | `tokio`, **`zbus`** (D-Bus puro-Rust) | serve a interface do daemon, o cliente `klippo toggle` e o canal extensão→daemon |
| UI | `gtk4` (gtk4-rs), `libadwaita`, **`relm4`** + `relm4-components` | estilo Elm pedido; `FactoryVecDeque` é ideal para lista MRU com botão de excluir por linha; `AdwStyleManager` segue claro/escuro do sistema |
| Captura X11 | `x11rb` (feature `xfixes`) + `arboard` (leitura pós-evento) | XFixes notifica troca de dono de PRIMARY/CLIPBOARD; arboard busca o valor |
| Captura Wayland data-control | `wayland-client` + `wayland-protocols-wlr` (+ XML ext-data-control via `wayland-scanner`) | KDE 6.4+/wlroots; *fallback*: `wl-paste --watch` |
| Escrita no clipboard | `wl-clipboard-rs` (Wayland), `arboard` (X11) | evita reimplementar posse de seleção |
| QR / "Show Barcode" | `qrcode` + `image` → `gdk::MemoryTexture` | renderiza o QR de um item |
| Actions (regex) | `regex` (padrão); `fancy-regex` atrás da feature `pcre-actions` | `regex` cobre a maioria; `fancy-regex` aproxima do QRegularExpression (lookahead/backref) |
| Execução de Actions | `tokio::process` + `shell-words` | substituição segura em argv (sem shell por padrão) |
| Armazenamento | **`rusqlite`** (SQLite *bundled*, WAL) | durável/transacional; consultas MRU/dedup/poda triviais; sem dep de sistema. JSON atrás da feature `json-store` |
| Config | `serde` + `toml`, `directories`, `notify` (recarregar) | espelha o `klipperrc` em TOML |
| Atalho KDE/wlroots | `ashpd` (portal GlobalShortcuts) | KDE implementa; GNOME **não** → usar `gsettings` |
| Base | `anyhow`/`thiserror`, `tracing`, `blake3` (hash p/ dedup) | — |

> Pinar um trio conhecido-bom de `gtk4` + `libadwaita` + `relm4` desde o início para evitar incompatibilidades.

---

## Estrutura do workspace (greenfield — `cwd` vazio)

Um único binário `klippo` com subcomandos (`daemon`, `toggle`, `show`, `hide`, `clear`, `setup`,
`extension install`). A UI vive **dentro** do processo daemon (o popup é uma janela que o daemon
mantém e faz `present()`/oculta via D-Bus) — sem segundo binário nem IPC extra só para exibir a lista.

```
rust/                                   (raiz do workspace = cwd)
├─ Cargo.toml                           # [workspace] + workspace.dependencies
├─ rust-toolchain.toml                  # pin 1.95.0
├─ data/
│  ├─ org.klippo.Daemon.service         # D-Bus activation
│  ├─ klippo-daemon.service             # systemd --user
│  ├─ klippo.desktop                    # autostart + entrada de app
│  ├─ style.css                         # CSS GTK imitando o Klipper
│  └─ fonts/JetBrainsMono-Regular.ttf   # fonte empacotada (OFL-1.1) + OFL.txt
├─ extension/                           # extensão GNOME Shell (GJS) — ponte de captura
│  ├─ metadata.json                     # "shell-version": ["50"] (Shell 50.1 verificado)
│  ├─ extension.js
│  └─ dbus.js
├─ crates/
│  ├─ klippo-core/   # lib SEM GUI/DBus: model, store(sqlite|json), search, config, actions, dedup
│  ├─ klippo-capture/# lib: trait ClipboardSource/Writer + backends + detecção de ambiente
│  ├─ klippo-dbus/   # lib: definições zbus das interfaces (compartilhada daemon+cliente)
│  └─ klippo/        # O binário: main(clap), daemon, client, setup, ui/{app,popup,row,search,settings,qr,edit}
└─ packaging/debian/                    # control, rules, postinst
```

`klippo-core` é livre de GUI/DBus → testes unitários rápidos de dedup/busca/actions/store.

---

## Arquitetura

**1. Trait de captura + seleção de backend no startup** (`klippo-capture`):

```rust
enum Source { Clipboard, Primary }
enum ClipboardEvent { Text{..}, Image{ mime, bytes, .. }, Cleared{..} }
trait ClipboardSource { async fn run(self, tx, cancel) -> Result<()>; fn name(&self)->&str; }
trait ClipboardWriter { fn set_text(..); fn set_image(..); }
```

Detecção (sobrescrevível por `KLIPPO_BACKEND=x11|wayland-dc|gnome` para testes):
- Wayland + desktop contém `gnome` → **GnomeBridgeSource** (eventos chegam pela extensão via D-Bus) + WaylandWriter. ← **sua máquina cai aqui.**
- Wayland + global data-control presente (ext/wlr) → **WaylandDataControlSource** (KDE 6.4+, Sway, Hyprland).
- X11 (ou `DISPLAY` setado) → **X11Source** (XFixes) + X11Writer (também testável via Xwayland).

**2. Interfaces D-Bus** (`org.klippo.Daemon` em `/org/klippo/Daemon`):
- `org.klippo.Daemon1` (controle/consulta p/ CLI e UI): `Toggle/Show/Hide/Clear`, `ListEntries`, `GetEntryContent`,
  `RemoveEntry(id)`, `Select(id)` (promove ao topo **e** copia p/ o clipboard — **não cola**, igual ao Klipper),
  `RunAction(id, action_id)`, `Get/SetConfig`. Sinais: `HistoryChanged`, `ConfigChanged`, `ActionPopupRequested`.
- `org.klippo.Capture1` (o que a extensão chama): `AddText(text, source)`, `AddImage(mime, bytes, source)`,
  `ClipboardCleared(source)`, `Heartbeat()`.
- Ship do `org.klippo.Daemon.service` → o D-Bus auto-ativa o daemon no primeiro `klippo toggle`.

**3. Extensão GNOME Shell (GJS) — ponte fina, nada além disso:**
- Conecta em `global.display.get_selection()` `owner-changed` (CLIPBOARD e PRIMARY).
- Lê via `St.Clipboard.get_default()` (texto; imagem `get_content(type,'image/png')` se `ignore_images=false`).
- **Debounce ~150–250 ms + dedup por hash** (colapsa o eco do próprio `Select()` do daemon).
- Envia via `Gio.DBusProxy` para `Capture1`; se o daemon estiver fora, descarta sem travar o Shell; `Heartbeat()` a cada 5 s.
- Instalação: `klippo extension install` copia p/ `~/.local/share/gnome-shell/extensions/klippo@klippo.org/` →
  `gnome-extensions enable` → **exige logout/login** (Wayland não recarrega extensão a quente — documentar).
- *Fase 2:* abrir no cursor reposicionando a janela do popup via `Meta.Window.move_frame(...)` perto de `global.get_pointer()`.

**4. UI (relm4 + GTK4 + libadwaita):** `AdwApplication`; o daemon constrói o popup uma vez e faz `present()`/oculta.
Ponte D-Bus(tokio)→UI por canal do `MainContext` do glib; toda chamada GTK na thread principal.

```
PopupWindow (AdwWindow, app-id org.klippo)
├─ SearchEntry (topo)                  -> Filter(query) em tempo real
├─ ScrolledWindow → ListBox via FactoryVecDeque<RowModel>   (DECRESCENTE / MRU)
│     └─ Row: [ícone/thumb] [preview com ellipsis] [espaço] [Excluir ✕]
└─ Rodapé: [Limpar tudo] [Configurações] [QR] [Editar]
SettingsDialog (AdwPreferencesWindow) · QrDialog (GtkPicture) · EditDialog (GtkTextView)
```
- **Busca:** o componente guarda o `Vec<Entry>` completo; `search-changed` recalcula o subconjunto (substring
  case-insensitive) e faz diff no factory; apagar restaura a lista. ↑/↓ navega, Enter = `Select`, Esc = `Hide`.
- **Tema:** `AdwStyleManager` segue o sistema (seu padrão é `prefer-dark` — validar claro **e** escuro);
  `data/style.css` em prioridade de aplicação p/ o visual compacto do Klipper.
- **JetBrains Mono:** o `.deb` instala a fonte em `/usr/share/fonts/truetype/jetbrains-mono/`; CSS usa
  `font-family: "JetBrains Mono", monospace` (Pango não carrega TTF por caminho arbitrário sem fontconfig).
- **Dispensar no Wayland:** sem foco global/posicionamento próprio → aproximar com `notify::is-active`→`Hide()`.

---

## Modelo de dados e armazenamento

- **Entry:** `id`, `kind`{Text,Image}, `text`/`image_path`+`thumb_path`, `preview`, `content_hash`, `timestamp`, `pinned`(futuro).
- **Dedup (fiel ao Klipper):** se o hash já existe, **atualiza timestamp e reordena ao topo** (não duplica). MRU = `ORDER BY timestamp DESC`.
- **Limite:** após inserir, apaga além de `max_items` (padrão **25**); coleta os PNG/thumb órfãos.
- **Locais:** DB `~/.local/share/klippo/history.db` (WAL); imagens `…/klippo/images/<hash>.png` (+ `thumbs/`); config `~/.config/klippo/config.toml`.
- **Persistência/restauração:** SQLite confirma a cada mudança; checkpoint do WAL em `Hide`/SIGTERM; migrações via `PRAGMA user_version`.

---

## Funcionalidades do clone (mapeadas do Klipper)

- **Histórico MRU**, mais recente no topo; selecionar copia p/ o clipboard (não cola) e sobe ao topo.
- **Busca incremental** (digitar filtra; apagar restaura) e **excluir item** por linha + **Limpar tudo**.
- **Claro/escuro** seguindo o sistema; **JetBrains Mono** como fonte padrão.
- **Config em TOML** espelhando o `klipperrc`, com os padrões do Klipper:
  `max_items=25`, `keep_clipboard_contents=true`, `sync_clipboards=false`, `ignore_selection=true`,
  `ignore_images=true`, `prevent_empty_clipboard=true`, `strip_whitespace=true`, `actions_enabled=true`,
  `timeout_for_action_popups=8`, `popup_at_cursor=false`.
- **Actions por regex:** lista de `[[actions]]` com `regex`, `commands` e `output`(ignore|replace-clipboard|new-entry);
  placeholders `%s`/`%0..%9` (grupos); `automatic=true` → menu auto (timeout 8 s); execução **sem shell por padrão**
  (split com `shell-words` + substituição em argv com aspas por argumento — clipboard malicioso como `; rm -rf ~` não injeta).
- **QR code** ("Show Barcode"), **Editar conteúdo** (dialog), **seleção×clipboard** (PRIMARY/CLIPBOARD + sync),
  **ignorar imagens**, **prevent-empty-clipboard**.

---

## Super+V / atalho global

**GNOME (alvo) — `gsettings` (`klippo setup keybinding`):**
1. Detectar conflito: `org.gnome.shell.keybindings toggle-message-tray = ['<Super>v','<Super>m']` → oferecer
   remover só `<Super>v` (mantém `<Super>m` p/ a bandeja).
2. `custom-keybindings` está vazio → adicionar caminho `…/custom-keybindings/klippo/` com
   `name='Klippo Toggle'`, `command='klippo toggle'`, `binding='<Super>v'`.

`klippo toggle` abre zbus e chama `Daemon1.Toggle()` (auto-ativa o daemon se preciso) — rápido, sem GTK.
**KDE:** portal GlobalShortcuts via `ashpd`. **X11/wlroots:** documentar bind `klippo toggle` no WM (Sway/i3/Hyprland/sxhkd).

---

## Empacotamento

**`.deb` nativo é o artefato principal** (via `cargo-deb`, instalar na P4): binário→`/usr/bin/klippo`; systemd
`--user` em `/usr/lib/systemd/user/`; D-Bus activation; autostart `.desktop`; fonte+OFL; extensão GNOME (cópia
por-usuário no primeiro `klippo setup`). Deps: `libgtk-4-1, libadwaita-1-0`. **Flatpak fica como secundário** — o
sandbox não escreve `gsettings` do GNOME nem instala extensão no host (quebra Super+V e a ponte de captura).

---

## Pré-requisitos de build (rodar antes das fases de UI)

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev   # headers ausentes (runtime já presente)
# x11rb/wayland-client são puro-Rust e provavelmente só precisam das libs de runtime (presentes);
# se a compilação dos backends nativos reclamar: sudo apt install libxcb1-dev libxcb-xfixes0-dev libwayland-dev
```
JetBrains Mono e `wl-clipboard` serão tratados pelo projeto/.deb; `cargo-deb` instala-se na P4.

---

## Roadmap em fases

- **P0 — Esqueleto + daemon + D-Bus + UI mínima + captura X11.** Workspace; `klippo-core` (model+SQLite+dedup+busca,
  com testes); `klippo-dbus`; daemon servindo `Daemon1`/`Capture1`; popup relm4 (busca + MRU + excluir ✕);
  backend **X11 XFixes** (testável via Xwayland sem a extensão).
- **P1 — Ponte GNOME + Super+V (MVP real aqui).** Extensão GJS capturando `owner-changed` (debounce/dedup) e
  empurrando `AddText`; `klippo extension install`; `klippo setup keybinding` (resolve o conflito do Super+V);
  `GnomeBridgeSource` + `WaylandWriter`.
- **P2 — Persistência + imagens + Configurações.** WAL/restore; imagens (extensão→`AddImage`, thumbnail) sob
  `ignore_images`; `AdwPreferencesWindow` com `SetConfig` ao vivo; claro/escuro.
- **P3 — Actions + QR + Editar.** Subsistema de actions (regex, `%s`/grupos, exec sem shell, auto-popup, `RunAction`);
  dialogs de QR e de edição; toggle global de actions.
- **P4 — KDE/wlroots data-control + polimento + empacotamento.** Backend `wayland_data_control` (ext+wlr, testar em VM);
  Super+V no KDE via `ashpd`; `.deb`; systemd/autostart/D-Bus activation; README (segurança + ressalvas Wayland); clippy/fmt/deny + CI.

---

## Verificação (ponta a ponta nesta máquina GNOME Wayland)

- **P0 (D-Bus/UI/X11):**
  `busctl --user call org.klippo.Daemon /org/klippo/Daemon org.klippo.Capture1 AddText ss "hello" "clipboard"`
  → `ListEntries` mostra `hello`; `klippo toggle` abre o popup; digitar filtra; apagar restaura; ✕ remove;
  `gdbus monitor` mostra `HistoryChanged`. `KLIPPO_BACKEND=x11 klippo daemon` + copiar num xterm → entra (valida XFixes).
- **P1 (sem X11):** instalar+habilitar a extensão, relogar; copiar texto em qualquer app GNOME → aparece em ~250 ms;
  Super+V alterna o popup (após rebind da bandeja); selecionar um item → vira o clipboard (colar p/ confirmar) e sobe ao topo.
- **P2:** copiar texto, `systemctl --user restart klippo-daemon`, `ListEntries` ainda mostra (persistência); com
  `ignore_images=false`, copiar um screenshot → linha com thumbnail; mudar `color_scheme` → tema acompanha.
- **P3:** Action `xdg-open %s` `automatic=true` em `https://example.com` → menu auto, abre navegador;
  **teste de injeção** `https://x; touch /tmp/pwned` → `/tmp/pwned` **não** pode existir; "Show Barcode" gera QR escaneável; "Editar" salva e atualiza o clipboard.
- **P4:** `.deb` em container/VM limpo → daemon autostart, extensão instala, Super+V após relog; em VM KDE/Sway, copiar externamente → captura via data-control (`KLIPPO_BACKEND=wayland-dc`).

---

## Principais riscos (validar cedo)

1. **Ponte da extensão + loop de eco (maior risco):** `owner-changed` dispara repetido e o próprio `Select()` re-dispara.
   Provar debounce + dedup-por-hash na P1; tratar extensão ausente / restart do Shell (Heartbeat + reconexão do proxy).
2. **Imagem via extensão:** disponibilidade/perf de `St.Clipboard.get_content('image/png')` no Shell 50 e PNGs grandes
   por D-Bus — limitar tamanho ou usar arquivo temporário. Como `ignore_images=true` é padrão, o MVP não depende disso.
3. **Foco/dispensar/posição no GNOME Wayland:** sem posicionamento próprio nem grab global → centralizado no MVP; cursor fica p/ a extensão (fase 2).
4. **Maturidade do data-control (KDE/wlroots):** ext-data-control é novo; *fallback* `wl-paste --watch`. (Risco só na P4.)
5. **JetBrains Mono (OFL-1.1):** permite redistribuir, mas exige enviar o OFL.txt e não reutilizar o nome reservado em modificações.
6. **Integração de 3 loops (tokio + glib/GTK + zbus):** fixar a ponte `MainContext`↔tokio na P0; GTK só na thread principal.

---

## Arquivos críticos a criar

- `crates/klippo-core/src/store/sqlite.rs` — persistência, dedup, ordem MRU, poda por limite, GC de imagens (coração dos dados)
- `crates/klippo-capture/src/lib.rs` — traits `ClipboardSource`/`Writer` + seleção de backend por ambiente
- `crates/klippo-dbus/src/lib.rs` — interfaces `Daemon1` + `Capture1` (compartilhadas daemon/CLI/extensão)
- `crates/klippo/src/ui/popup.rs` — popup relm4: busca, `FactoryVecDeque` MRU, excluir por linha, tema, show/hide via D-Bus
- `extension/extension.js` — ponte GNOME Shell: captura `owner-changed`, debounce/dedup, push D-Bus (caminho de captura obrigatório do MVP)
