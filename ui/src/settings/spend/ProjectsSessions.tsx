// Ported from upstream Settings/TokenSpendView.swift (`projects`, `sessions`, `sessionSubtitle`): where the work
// happened. One row per project identity, heaviest first, and one row per transcript, newest first, paged.
import { useState } from "react";
import { AgentIcon } from "../../panel/Icon";
import { t } from "../../shared/i18n";
import {
  dateTime, shortDate, sessionLabel, sessionNamesItself, tokensText,
  type ProjectRow, type SessionRow,
} from "../../shared/spend";
import { Group, Row } from "../Group";
import { CostText, PAGE_SIZES, pageOf, ShareBar, SpendPageFooter } from "./SpendCharts";

/** Enough to see where the work goes without turning the pane into a table. */
export const LIST_LIMIT = 8;

/** The money to show, or null where none of the tokens had a price: an unpriced project is not a free one. */
const estimated = (r: { tokens: number; unpricedTokens: number; cost: number }) =>
  r.tokens > 0 && r.unpricedTokens === r.tokens ? null : r.cost;

/** One row per project identity, with ambiguous directory names expanded. */
export function ProjectsGroup({ projects, total: spanTokens }: { projects: ProjectRow[]; total: number }) {
  // Of all the work, not of the work with a project: sessions with no directory (Codex's path carries none)
  // have no row here, and dividing by the projects alone drew each bar larger than its share beside every
  // other section's.
  const total = Math.max(spanTokens, 1);
  return (
    <Group title={t("By project")}>
      {projects.slice(0, LIST_LIMIT).map((project, i) => (
        <Row
          key={`${i}|${project.name}`}
          title={project.name}
          subtitle={t("%@ sessions · last used %@", project.sessions, shortDate(project.lastUsed))}
        >
          <ShareBar share={project.tokens / total} />
          <span className="nowrap num small-cost"><CostText cost={estimated(project)} unpriced={project.unpricedTokens} /></span>
        </Row>
      ))}
    </Group>
  );
}

/** When it ran, and the project beside it where the row's title is not already the directory. */
function sessionSubtitle(row: SessionRow): string {
  const when = dateTime(row.end);
  return row.project && sessionNamesItself(row) ? `${when} · ${row.project}` : when;
}

/** One row a transcript, newest first. */
export function SessionsGroup({ sessions }: { sessions: SessionRow[] }) {
  // The session list pages by itself: it is hundreds of rows where the day table is tens, and a shared page
  // number would jump both at once.
  const [pageSize, setPageSize] = useState<number>(PAGE_SIZES[0]);
  const [page, setPage] = useState(0);
  const current = pageOf(sessions.length, pageSize, page);
  const shown = sessions.slice(current * pageSize, (current + 1) * pageSize);
  return (
    <Group title={t("By session")}>
      {shown.map((row, i) => (
        <Row
          key={`${current}|${i}`}
          // What the conversation was called: the directory is the fallback, then "Untitled conversation",
          // never the transcript's file name.
          title={sessionLabel(row, row.project)}
          subtitle={sessionSubtitle(row)}
          icon={<AgentIcon agent={row.agent} size={18} />}
        >
          <span className="secondary small nowrap num">{tokensText(row.tokens)}</span>
          <span className="nowrap num small-cost"><CostText cost={estimated(row)} unpriced={row.unpricedTokens} /></span>
        </Row>
      ))}
      {sessions.length > PAGE_SIZES[0] && (
        <SpendPageFooter rows={sessions.length} pageSize={pageSize} page={current} onPageSize={setPageSize} onPage={setPage} />
      )}
    </Group>
  );
}
