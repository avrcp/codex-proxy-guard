# Product

<!-- impeccable:product-schema 1 -->

## Platform

Windows console application (Win32 VT TUI with a plain-console fallback).

## Stack

C++20, STL, Windows SDK, MSVC x64, CMake; single executable, in-process
engine session, static CRT.

## Users and purpose

Windows users launch ChatGPT Desktop with a local HTTP/Mixed proxy. Guard provides
explicit proxy configuration and scoped backend consent, with occasional repair.
It does not manage proxy services, measure connectivity or own Desktop's lifecycle.

## Constraints

C++ is the only implementation of system operations. One foreground operation,
explicit consent, bounded cancellation, no network probes, no authentication data.
An observed activation or prepared configuration is never a network success claim.

## Brand commitments

A compact, quiet console launcher: one screen that answers which proxy, which
application, whether it is running and what proxy configuration is prepared.
Launch is the only primary action; destructive confirmations require typing YES.
Product copy stays in English; status is always stated in text in addition to
color.

## Accessibility

Keyboard access for every task (Enter/E/B/D/R/?, Ctrl+C cancel, Tab field
switching); text in addition to status colors; the plain line console is a
full fallback when VT output is unavailable.
