import sessionData from "./session.json";
import { visibleEvents, type MeetupEvent } from "./events";

export const currentUser = sessionData.user;

const attendingIds = new Set(sessionData.attendingEventIds);
const hostedIds = new Set(sessionData.hostedEventIds);

export const attendingEvents: MeetupEvent[] = visibleEvents.filter((event) =>
  attendingIds.has(event.id),
);

export function isHostedEvent(eventId: string): boolean {
  return hostedIds.has(eventId);
}
