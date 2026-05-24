# Klippo

Um gerenciador de histórico da área de transferência para Linux, inspirado no
**Klipper** do KDE, mas pensado para o **Ubuntu/GNOME** e demais distribuições —
funcionando em **Wayland** e **X11**. Núcleo e interface em **Rust** (GTK4 +
libadwaita); no GNOME Wayland a captura é feita por uma pequena extensão do
GNOME Shell que conversa com o daemon via D-Bus.

> Por que a extensão? No GNOME Wayland o compositor (Mutter) não implementa os
> protocolos `wlr-data-control`/`ext-data-control`, então **nenhum app externo
> consegue monitorar a área de transferência**. Só código rodando dentro do
> GNOME Shell consegue — como fazem GPaste, Pano e Clipboard Indicator.

## Estado atual

**Funciona e está testado:**
- Núcleo (`klippo-core`): modelo, armazenamento SQLite (WAL), deduplicação por
  hash + ordem MRU, poda por limite, busca incremental, configuração TOML
  (espelhando o `klipperrc`), subsistema de Actions por regex. *14 testes.*
- Serviço D-Bus (`org.klippo.Daemon`): interfaces `Daemon1` (controle/consulta) e
  `Capture1` (recepção de capturas). Verificado via `busctl`.
- Popup GTK4 + libadwaita: busca em tempo real, lista decrescente (recente no
  topo), botões por item que aparecem ao passar o mouse (executar ação / QR /
  editar / remover), "limpar tudo", miniaturas para imagens, claro/escuro
  seguindo o sistema, abrir/ocultar/alternar via D-Bus.
- Diálogos: **Configurações** (`AdwPreferencesWindow` — limite, ignorar imagens,
  sincronizar seleção, tema, etc.), **Editar conteúdo**, **Mostrar QR**, e menu de Ações.
- Extensão GNOME Shell: captura texto **e imagens** (quando habilitado) no
  Wayland; `klippo setup` (Super+V via gsettings + extensão + fonte JetBrains Mono).
- Actions por regex: execução **sem shell** (à prova de injeção — verificado),
  `%s`/`%0..%9`, `RunAction` por D-Bus, auto-popup, e execução pela UI.
- Imagens: `AddImage` armazena PNG + miniatura; exibidas na lista. Verificado.
- Captura direta: **X11** (polling via arboard) e **Wayland data-control**
  (`wl-paste --watch` → `klippo __feed`) para KDE 6.4+/wlroots. Inicialização
  verificada; o caminho `__feed` verificado de ponta a ponta.
- Persistência entre reinícios. Empacotamento `.deb` (gera artefato instalável). Verificado.

**Refinamentos futuros (precisam do seu olho / de outros ambientes):**
- Ajustes visuais do popup (espaçamentos, ícones, dimensões) após você ver rodando.
- Testar X11/Wayland-data-control em sessões reais (Xubuntu/Kubuntu/Sway).
- Abrir no cursor no GNOME Wayland (via a extensão movendo a janela), ícone do app,
  fixar itens (pin), e sincronização ativa seleção↔área de transferência.

## Arquitetura

```
Extensão GNOME (GJS) ──D-Bus(Capture1)──┐
                                        ▼
X11 / KDE / wlroots ──captura direta──► NÚCLEO RUST (daemon)
   (a implementar)                      • SQLite, dedup/MRU, busca
                                        • config TOML, Actions (regex)
                                        • serviço D-Bus
                                                │ async-channel (UiEvent)
                                                ▼
                                        UI GTK4 + libadwaita
                                   Super+V → gsettings → `klippo toggle`
```

Crates: `klippo-core` (lógica pura, sem GUI/D-Bus), `klippo-capture` (traits +
detecção de backend), `klippo-dbus` (interfaces zbus compartilhadas), `klippo`
(binário: daemon + UI + CLI + setup).

## Requisitos

- Rust 1.95+ e um compilador C (para o SQLite empacotado).
- GTK4 e libadwaita de desenvolvimento: `sudo apt install libgtk-4-dev libadwaita-1-dev`.
- GNOME Shell (para a extensão de captura no Wayland).

## Compilar

```bash
cargo build --release          # binário em target/release/klippo
cargo test                     # roda os testes do núcleo
```

## Rodar (desenvolvimento)

```bash
# 1. Inicie o daemon (captura + histórico + UI + D-Bus):
target/release/klippo daemon &

# 2. Abra o popup:
target/release/klippo toggle        # alterna mostrar/ocultar
target/release/klippo show          # mostra
target/release/klippo clear         # limpa o histórico
```

No GNOME Wayland, a captura só funciona depois de instalar a extensão e
**reiniciar a sessão** (veja abaixo). Antes disso, dá para testar inserindo itens
via D-Bus:

```bash
busctl --user call org.klippo.Daemon /org/klippo/Daemon \
  org.klippo.Capture1 AddText ss "olá mundo" "clipboard"
```

## Ativar no GNOME (atalho Super+V + captura)

```bash
target/release/klippo setup
```

Isso: instala a fonte JetBrains Mono em `~/.local/share/fonts`, copia/ativa a
extensão `klippo@klippo.org`, e religa **Super+V** para `klippo toggle` (liberando
o atalho do "toggle-message-tray" do GNOME; a bandeja continua em **Super+M**).
Depois, **faça logout/login** para o GNOME Shell carregar a extensão. Então copie
algo e pressione **Super+V**.

Comandos individuais: `klippo install-extension`, `klippo keybinding`.

## Configuração

`~/.config/klippo/config.toml` (gerado com padrões do Klipper na primeira
execução). Principais opções: `max_items` (padrão 25), `ignore_images`,
`ignore_selection`, `sync_clipboards`, `prevent_empty_clipboard`,
`actions_enabled`, e a aparência (`color_scheme`, `font_family`, `popup_width`).
As Actions ficam em blocos `[[actions]]` (regex + comandos com `%s`/`%1`).

Histórico em `~/.local/share/klippo/history.db`.

## Empacotar (.deb)

```bash
cargo install cargo-deb
cargo deb -p klippo
```

Instala o binário, a unidade systemd `--user`, o autostart, a ativação D-Bus, a
fonte e a extensão.

## Desinstalar / reverter o atalho

```bash
gnome-extensions disable klippo@klippo.org
rm -rf ~/.local/share/gnome-shell/extensions/klippo@klippo.org
gsettings reset org.gnome.shell.keybindings toggle-message-tray
# remova o custom-keybinding 'klippo' em Configurações → Teclado → Atalhos
```

## Licença

GPL-3.0-or-later. Inclui a fonte JetBrains Mono (OFL-1.1, veja `data/fonts/OFL.txt`).
