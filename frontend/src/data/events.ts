import eventsData from "./events.json";

export type MeetupEvent = {
  id: string;
  emoji: string;
  title: string;
  signedCount: number;
  capacity: number;
  latitude: number;
  longitude: number;
  hostName: string;
};

export const events = eventsData as MeetupEvent[];

export const visibleEvents = events.filter(
  (event) => event.signedCount < event.capacity,
);
