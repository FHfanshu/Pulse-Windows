// One card of a deck, chosen by id (upstream RecapCardView).
import type { Deck } from "../deck";
import type { CardId } from "../types";
import { CalendarCard, MonthsCard, YearCalendarCard } from "./Calendar";
import { OpenerCard } from "./Opener";
import { PaybackCard } from "./Payback";
import { PosterCard } from "./Poster";
import { ScorecardCard } from "./Scorecard";
import { TimetableCard } from "./Timetable";

export function CardView({ deck, card }: { deck: Deck; card: CardId }) {
  switch (card) {
    case "poster":
      return <PosterCard deck={deck} />;
    case "opener":
      return <OpenerCard deck={deck} />;
    case "calendar":
      return <CalendarCard deck={deck} />;
    case "yearCalendar":
      return <YearCalendarCard deck={deck} />;
    case "months":
      return <MonthsCard deck={deck} />;
    case "timetable":
      return <TimetableCard deck={deck} />;
    case "payback":
      return <PaybackCard deck={deck} />;
    case "scorecard":
      return <ScorecardCard deck={deck} />;
  }
}
