# User installation by default. DESTDIR is for package staging.
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
CARGO ?= cargo

.PHONY: build install
build:
	$(CARGO) build --release --locked --manifest-path apps/desktop/Cargo.toml

install: build
	install -Dm755 apps/desktop/target/release/cognitive-popups "$(DESTDIR)$(BINDIR)/cognitive-popups"
	install -d "$(DESTDIR)$(DATADIR)/cognitive-popups/prompts"
	install -m644 docs/v2/port/prompts/*.md "$(DESTDIR)$(DATADIR)/cognitive-popups/prompts/"
	install -Dm644 packaging/cognitive-popups.desktop "$(DESTDIR)$(DATADIR)/applications/cognitive-popups.desktop"
	apps/desktop/target/release/cognitive-popups hyprland-config --classic > "$(DESTDIR)$(DATADIR)/cognitive-popups/hyprland.conf"
	apps/desktop/target/release/cognitive-popups hyprland-config --lua > "$(DESTDIR)$(DATADIR)/cognitive-popups/hyprland.lua"
	@printf '%s\n' 'Установка завершена. Выведите настройки клавиш: cognitive-popups hyprland-config --lua (или --classic).'
