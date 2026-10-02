# Product

<!-- impeccable:product-schema 1 -->

## Platform

Windows native desktop (Qt Widgets).

## Stack

C++20, Qt 6.8+ Core/Gui/Widgets, MSVC x64, CMake; existing Rust engine over stdio.

## Users and purpose

Windows users launch ChatGPT Desktop with a local HTTP/Mixed proxy. Guard provides
explicit proxy configuration and scoped backend consent, with occasional repair.
It does not manage proxy services, measure connectivity or own Desktop's lifecycle.

## Constraints

Rust is the only implementation of system operations. One foreground operation,
explicit consent, bounded cancellation, no network probes, no authentication data.
An observed activation or prepared configuration is never a network success claim.

## Brand commitments

The supplied brief pins a compact, quiet, Telegram-inspired native launcher:
single window, native title bar, standard accessible controls, system light/dark.
No Telegram code, branding, icons or palette resources. Product copy follows the
existing English UI. This record derives from the explicit implementation brief.

## Accessibility

Keyboard access and visible buttons for every task; labeled controls; text in
addition to status colors; DPI-aware layouts and readable long-path dialogs.
