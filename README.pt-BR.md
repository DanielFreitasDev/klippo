# Klippo

**Gerenciador de histórico da área de transferência para Linux, no estilo do
Klipper (KDE).** Feito em Rust com GTK4/libadwaita, funcionando em **Wayland** e
**X11**, com suporte de primeira no **Ubuntu/GNOME**.

[English 🇺🇸](README.md) · **Português**

O Klippo mantém um histórico pesquisável de tudo o que você copia — texto e
imagens — e abre no cursor com **Super+V**, igual ao Klipper do KDE, mas pensado
para se integrar bem ao GNOME e a outros desktops Linux.

> **Por que uma extensão do GNOME Shell?** No GNOME Wayland o compositor (Mutter)
> não implementa os protocolos `wlr-data-control` nem `ext-data-control`, então
> **nenhum aplicativo externo consegue monitorar a área de transferência**.
> Apenas código rodando dentro do GNOME Shell consegue — é assim que o GPaste, o
> Pano e o Clipboard Indicator funcionam. O Klippo inclui uma pequena extensão
> que captura as cópias e as envia ao daemon Rust via D-Bus. No X11, no KDE
> Plasma e em compositores wlroots o daemon captura diretamente e a extensão não
> é necessária.

## Funcionalidades

- 📋 **Histórico de texto e imagens** — mais recente no topo, deduplicação
  automática, tamanho configurável (padrão **25**), copia textos ou imagens de
  volta para a área de transferência e persiste entre reinícios.
- 🔍 **Busca em tempo real** — digite para filtrar; apague para voltar à lista
  completa.
- ⌨️ **Itens numerados** (1–9) com seleção rápida por **Alt+1…Alt+9**; setas e
  Enter também funcionam.
- 🖱️ **Abre no cursor** e **fecha ao perder o foco** (clicar fora), no estilo
  Klipper. *(O posicionamento no cursor, no GNOME Wayland, é feito pela
  extensão — veja a tabela de suporte.)*
- 🧰 **Botões por item** que aparecem ao passar o mouse: executar uma ação,
  mostrar **QR code**, **editar** ou **remover** o item — além de **Limpar tudo**.
- 🎨 **Claro/escuro** seguindo o tema do sistema, com **JetBrains Mono** embutida.
- ⚙️ **Janela de Configurações** — tamanho do histórico, ignorar imagens, ignorar
  seleção do mouse, sincronizar seleção↔área de transferência, evitar área vazia,
  ativar/desativar ações, tema.
- 🤖 **Ações por regex** (como o Klipper) — casa o texto copiado e executa
  comandos com `%s` / `%0`–`%9`. **Sem shell por padrão** (à prova de injeção),
  com menu automático opcional.

## Ambientes suportados

| Desktop / sessão | Captura | Super+V | Abrir no cursor |
|---|---|---|---|
| **GNOME (Wayland)** — Ubuntu, Fedora, … | ✅ extensão do GNOME Shell (texto + imagens PNG) | ✅ configurado automaticamente | ✅ |
| **X11** (qualquer desktop) | ✅ polling de CLIPBOARD + PRIMARY (texto + imagens) | ⚙️ vincule `klippo toggle` | ➖ posicionado pelo WM |
| **KDE Plasma 6 / wlroots** (Sway, Hyprland) | ✅ `wl-paste --watch` para texto + imagens PNG (requer `wl-clipboard`) | ⚙️ vincule `klippo toggle` | ➖ posicionado pelo compositor |

> O alvo principal de desenvolvimento e testes é o **GNOME no Wayland (Ubuntu)**.
> Os backends de X11 e de Wayland data-control estão implementados; testes nessas
> sessões são muito bem-vindos.

## Instalação

### Pelo `.deb` (Debian/Ubuntu)

```bash
cargo install cargo-deb          # uma vez
cargo deb -p klippo              # gera target/debian/klippo_*_amd64.deb
sudo dpkg -i target/debian/klippo_*_amd64.deb
```

O pacote instala o binário, um serviço systemd de **usuário**, um arquivo de
ativação D-Bus, uma entrada de autostart e a fonte JetBrains Mono embutida.

### A partir do código

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev   # dependências de build
cargo build --release                            # → target/release/klippo
```

## Configuração inicial (GNOME)

```bash
klippo setup
```

Isso instala a fonte JetBrains Mono, instala e ativa a extensão do GNOME Shell e
vincula **Super+V** a `klippo toggle` (liberando o `<Super>v` do atalho de bandeja
de notificações do GNOME, que continua em `<Super>m`). Depois, **faça logout e
login** para o GNOME Shell carregar a extensão. Copie algo e pressione **Super+V**.

Comandos granulares: `klippo install-extension`, `klippo keybinding`.

No **X11** ou em **wlroots/KDE**, a extensão não é necessária — basta vincular um
atalho a `klippo toggle` nas configurações do desktop/WM e garantir que o
`klippo daemon` inicie no login (o `.deb` já faz o autostart).

## Uso

| Ação | Como |
|---|---|
| Abrir / fechar o popup | **Super+V** (ou `klippo toggle`) |
| Filtrar | comece a digitar |
| Escolher um item | clique, **Enter** (primeiro resultado) ou **Alt+1…9** |
| Ação / QR / editar / remover (por item) | passe o mouse na linha |
| Limpar tudo · Configurações | botões no rodapé |
| Dispensar | **Esc** ou clicar fora |

Selecionar um item copia seu conteúdo (texto ou imagem) para a área de
transferência (**não** cola automaticamente) e o move para o topo — igual ao
Klipper.

## Configuração

A configuração fica em `~/.config/klippo/config.toml` (criada com padrões à la
Klipper na primeira execução); o histórico fica em
`~/.local/share/klippo/history.db`.

```toml
[general]
max_items = 25
keep_clipboard_contents = true
ignore_images = true
ignore_selection = true          # não captura seleções do mouse (PRIMARY)
selection_text_only = true       # seleções PRIMARY só armazenam texto
sync_clipboards = false
prevent_empty_clipboard = true
actions_enabled = true

[ui]
color_scheme = "system"          # system | light | dark
font_family = "JetBrains Mono"
popup_width = 380
popup_max_rows = 12

# Exemplo de ação por regex — executa sem shell (à prova de injeção):
[[actions]]
name = "Abrir URL"
regex = '^(https?://\S+)$'
automatic = false                # true = abre o menu de ações automaticamente
  [[actions.commands]]
  command = "xdg-open %s"        # %s = casamento inteiro, %1..%9 = grupos
  output = "ignore"              # ignore | replace-clipboard | new-entry
```

## Arquitetura

O Klippo é um workspace Cargo com quatro crates mais uma extensão do GNOME Shell:

```
Extensão GNOME Shell (GJS) ──D-Bus (Capture1)──┐
  captura o clipboard no Wayland               ▼
X11 / KDE / wlroots ── captura direta ───►  klippo-core (daemon)
  (arboard / wl-paste)                      • histórico SQLite, dedup + MRU
                                            • config TOML, Ações (regex)
                                            • serviço zbus (org.klippo.Daemon)
                                                    │ async-channel (UiEvent)
                                                    ▼
                                            popup GTK4 + libadwaita
                                    Super+V → gsettings → `klippo toggle`
```

- **`klippo-core`** — modelo, store SQLite (dedup + MRU + poda), busca, config
  TOML, Ações por regex. Sem GUI/D-Bus; com testes unitários.
- **`klippo-dbus`** — interfaces zbus compartilhadas: `Daemon1`
  (controle/consulta) e `Capture1` (recepção de capturas).
- **`klippo-capture`** — traits `ClipboardSource`/`ClipboardWriter`, detecção de
  ambiente e os backends X11 / Wayland-data-control / ponte-GNOME.
- **`klippo`** — o binário: daemon, popup GTK4, CLI e o `setup` do GNOME.
- **`extension/`** — a ponte do GNOME Shell (captura + posicionamento no cursor).

## Desenvolvimento

```bash
cargo test --workspace      # testes unitários
cargo clippy --workspace    # lints
cargo fmt --all             # formatação
```

## Desinstalação

```bash
sudo apt remove klippo                       # o pacote
# extensão GNOME + atalho (se você rodou `klippo setup`):
gnome-extensions disable klippo@klippo.org
rm -rf ~/.local/share/gnome-shell/extensions/klippo@klippo.org
gsettings reset org.gnome.shell.keybindings toggle-message-tray
# depois remova a entrada "klippo" em Configurações → Teclado → Atalhos personalizados
```

## Licença

Licenciado sob [GPL-3.0-or-later](https://www.gnu.org/licenses/gpl-3.0.html).
Inclui a fonte **JetBrains Mono** sob a SIL Open Font License 1.1
(`data/fonts/OFL.txt`).

## Créditos

Inspirado no **Klipper** do KDE. Fonte monoespaçada pela **JetBrains**.
