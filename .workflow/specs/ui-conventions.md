---
title: UI Conventions
readMode: optional
priority: medium
category: ui
keywords:
  - ui
  - design
  - color
  - typography
  - layout
  - animation
  - component
related:
  - "spec:project:coding-conventions"
---


# UI Conventions

Auto-generated from project analysis. Update manually as patterns evolve.

## Framework
- React 19 + TypeScript 6
- Build tool: Vite 8
- Styling: Tailwind CSS v4 (PostCSS plugin)

## Component Library
- Base primitives: Radix UI (dialog, checkbox, switch, dropdown-menu, toast, tooltip)
- Custom UI components: `admin-ui/src/components/ui/` (button, card, badge, input, dialog, etc.)
- Utility: `class-variance-authority` for variant styling, `clsx` + `tailwind-merge` for class merging
- Icons: lucide-react
- Toasts: sonner

## State Management
- Server state: TanStack React Query v5
- HTTP client: Axios
- Local storage: custom `admin-ui/src/lib/storage.ts`

## File Organization
- Pages/features: `admin-ui/src/components/` (flat, feature-named)
- Shared UI: `admin-ui/src/components/ui/`
- Hooks: `admin-ui/src/hooks/`
- API layer: `admin-ui/src/api/`
- Types: `admin-ui/src/types/`
- Utilities: `admin-ui/src/lib/`

## Naming
- Components: PascalCase (`credential-card.tsx` → `CredentialCard`)
- Files: kebab-case
- Hooks: `use-<name>.ts`

## Entries

