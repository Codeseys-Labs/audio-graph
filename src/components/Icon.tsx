/**
 * Icon registry (ADR-0010).
 *
 * Single source of truth mapping stable, semantic icon names to lucide-react
 * components. Component code references names (e.g. `<Icon name="close" />`),
 * never lucide imports directly, so the icon set is curated in one place and
 * swaps are trivial. Named lucide imports are tree-shaken by Vite.
 *
 * Icons render as inline SVG using `currentColor`, so they inherit the design
 * tokens (ADR-0009) from whatever text color is in scope and can express
 * state (active/disabled/error) via CSS — unlike the emoji they replaced.
 *
 * Accessibility: icons are decorative by default (`aria-hidden`). Pass a
 * `title` to expose an accessible name (renders `role="img"`); for interactive
 * glyphs prefer `<IconButton>`, which owns the accessible name on the button.
 */
import {
  X,
  TriangleAlert,
  FileText,
  MessageSquare,
  Play,
  Square,
  Headphones,
  Bot,
  ChartColumn,
  Settings,
  Mic,
  RefreshCw,
  Users,
  Search,
  Share2,
  Monitor,
  Volume2,
  AppWindow,
  Package,
  FolderTree,
  Check,
  Send,
  Trash2,
  NotebookPen,
  FlaskConical,
  ArrowRight,
  ChevronRight,
  ChevronDown,
  Maximize,
  Download,
  Info,
  CircleCheck,
  CircleAlert,
  type LucideIcon,
} from "lucide-react";

export const ICONS = {
  close: X,
  warning: TriangleAlert,
  transcript: FileText,
  chat: MessageSquare,
  start: Play,
  stop: Square,
  headphones: Headphones,
  agent: Bot,
  tokens: ChartColumn,
  settings: Settings,
  mic: Mic,
  refresh: RefreshCw,
  resample: RefreshCw,
  diarization: Users,
  extraction: Search,
  graph: Share2,
  system: Monitor,
  speaker: Volume2,
  apps: AppWindow,
  package: Package,
  processes: FolderTree,
  check: Check,
  send: Send,
  trash: Trash2,
  notes: NotebookPen,
  demo: FlaskConical,
  arrowRight: ArrowRight,
  chevronRight: ChevronRight,
  chevronDown: ChevronDown,
  fit: Maximize,
  download: Download,
  info: Info,
  success: CircleCheck,
  error: CircleAlert,
} satisfies Record<string, LucideIcon>;

export type IconName = keyof typeof ICONS;

export interface IconProps {
  name: IconName;
  /** Pixel size of the square glyph. Defaults to 16 (1em-ish at base font). */
  size?: number;
  /** Stroke width; lucide default is 2. */
  strokeWidth?: number;
  className?: string;
  /**
   * When set, the icon is exposed to assistive tech with this accessible
   * name (`role="img"`). When omitted (default) the icon is `aria-hidden`.
   */
  title?: string;
}

/**
 * Render a registered icon. Decorative by default; pass `title` for a
 * standalone meaningful icon. For buttons, use `<IconButton>`.
 */
export default function Icon({
  name,
  size = 16,
  strokeWidth = 2,
  className,
  title,
}: IconProps) {
  const Glyph = ICONS[name];
  return (
    <Glyph
      size={size}
      strokeWidth={strokeWidth}
      className={className}
      aria-hidden={title ? undefined : true}
      role={title ? "img" : undefined}
      aria-label={title}
    />
  );
}
