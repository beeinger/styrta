import { usersIconSvg } from "../icons";
import { colors } from "../theme";

export type MapMarker = {
  id: string;
  latitude: number;
  longitude: number;
  title: string;
  emoji: string;
  hostName: string;
  placeName: string;
  signedCount: number;
  capacity: number | null;
  startsAtLabel: string;
};

export type MapPadding = {
  top: number;
  right: number;
  bottom: number;
  left: number;
};

type MapDocumentOptions = {
  latitude: number;
  longitude: number;
  zoom: number;
  minZoom: number;
  maxZoom: number;
};

export function createMapHtml(options: MapDocumentOptions): string {
  const config = JSON.stringify(options);

  return `<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta
      name="viewport"
      content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no"
    />
    <link
      rel="stylesheet"
      href="https://fonts.googleapis.com/css2?family=Nunito:wght@400;600&display=swap"
    />
    <link
      rel="stylesheet"
      href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css"
    />
    <style>
      html,
      body,
      #map {
        height: 100%;
        width: 100%;
        margin: 0;
        padding: 0;
        background: ${colors.canvas};
      }
      #map {
        touch-action: none;
      }
      .leaflet-container {
        background: ${colors.canvas};
        font-family: Nunito, sans-serif;
      }
      .leaflet-bottom.leaflet-left {
        margin-bottom: var(--map-pad-bottom, 0px);
        margin-left: var(--map-pad-left, 8px);
      }
      .leaflet-control-attribution {
        font-size: 11px;
      }
      .pin-wrap {
        background: none !important;
        border: none !important;
        overflow: visible !important;
      }
      .event-marker {
        position: relative;
        width: 48px;
        height: 48px;
      }
      .event-bubble {
        width: 48px;
        height: 48px;
        border-radius: 50%;
        background: ${colors.glass};
        box-shadow: 0 8px 20px rgba(28, 28, 30, 0.12);
        border: 1px solid rgba(255, 255, 255, 0.9);
        box-sizing: border-box;
        display: flex;
        align-items: center;
        justify-content: center;
        font-size: 24px;
        line-height: 1;
      }
      .event-marker.is-selected .event-bubble {
        background: ${colors.glassSelected};
      }
      .event-card {
        display: none;
        position: absolute;
        left: 50%;
        bottom: calc(100% + 10px);
        transform: translateX(-50%);
        width: max-content;
        min-width: 168px;
        max-width: 228px;
        background: ${colors.white};
        border-radius: 16px;
        padding: 12px 14px 11px;
        box-shadow:
          0 12px 28px rgba(28, 28, 30, 0.14),
          0 0 0 1px rgba(28, 28, 30, 0.06);
        text-align: left;
        pointer-events: auto;
      }
      .event-card::after {
        content: "";
        position: absolute;
        left: 50%;
        bottom: -5px;
        width: 10px;
        height: 10px;
        background: ${colors.white};
        border-right: 1px solid rgba(28, 28, 30, 0.06);
        border-bottom: 1px solid rgba(28, 28, 30, 0.06);
        transform: translateX(-50%) rotate(45deg);
      }
      .event-marker.is-selected .event-card {
        display: block;
      }
      .event-title,
      .event-time,
      .event-place {
        margin: 0;
        overflow-wrap: anywhere;
      }
      .event-title {
        color: ${colors.ink};
        font-size: 15px;
        font-weight: 600;
        line-height: 20px;
      }
      .event-time,
      .event-place {
        margin-top: 4px;
        color: ${colors.inkMuted};
        font-size: 13px;
        font-weight: 400;
        line-height: 18px;
      }
      .event-place {
        color: ${colors.ink};
      }
      .event-count {
        display: flex;
        align-items: center;
        gap: 6px;
        margin: 8px 0 0;
        color: ${colors.ink};
        font-size: 13px;
        font-weight: 600;
        line-height: 18px;
      }
      .event-count span {
        display: flex;
        flex: none;
      }
      .event-count svg {
        display: block;
      }
      .user-location {
        position: absolute;
        left: 50%;
        top: 50%;
        width: 18px;
        height: 18px;
        transform: translate(-50%, -50%);
        border: 2px solid #ffffff;
        border-radius: 50%;
        box-sizing: border-box;
        background: #0a84ff;
        box-shadow: 0 0 0 7px rgba(10, 132, 255, 0.35);
      }
    </style>
  </head>
  <body>
    <div id="map"></div>
    <script>
      const INITIAL = ${config};
      const USERS_ICON = ${JSON.stringify(usersIconSvg(colors.ink, 15))};

      function post(message) {
        if (window.ReactNativeWebView) {
          window.ReactNativeWebView.postMessage(JSON.stringify(message));
        }
      }

      function fail(message) {
        post({ type: "error", message: message });
      }
    </script>
    <script
      src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"
      onerror="fail('Leaflet failed to load')"
    ></script>
    <script>
      (function () {
        if (!window.L) {
          fail("Leaflet failed to load");
          return;
        }

        let padding = { top: 0, right: 0, bottom: 0, left: 0 };
        let appliedOffset = L.point(0, 0);
        let selectedId = null;
        let zoomSource = "gesture";
        let suppressMapClick = false;
        let userMarker = null;
        const markerLayer = L.layerGroup();

        const map = L.map("map", {
          zoomControl: false,
          attributionControl: false,
          minZoom: INITIAL.minZoom,
          maxZoom: INITIAL.maxZoom,
          zoomSnap: 1,
          zoomDelta: 1,
          tapTolerance: 15,
        });

        L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
          maxZoom: 19,
          attribution: "&copy; OpenStreetMap",
        }).addTo(map);

        L.control
          .attribution({ position: "bottomleft", prefix: false })
          .addAttribution("&copy; OpenStreetMap")
          .addTo(map);

        map.setView([INITIAL.latitude, INITIAL.longitude], INITIAL.zoom);

        function paddingOffset() {
          const size = map.getSize();
          const geometric = L.point(size.x / 2, size.y / 2);
          const visual = L.point(
            (padding.left + size.x - padding.right) / 2,
            (padding.top + size.y - padding.bottom) / 2,
          );
          return visual.subtract(geometric);
        }

        function applyPadding(animate) {
          document.documentElement.style.setProperty(
            "--map-pad-bottom",
            padding.bottom + "px",
          );
          document.documentElement.style.setProperty(
            "--map-pad-left",
            padding.left + "px",
          );
          const target = paddingOffset();
          const delta = target.subtract(appliedOffset);
          appliedOffset = target;
          if (delta.x || delta.y) {
            map.panBy(delta, { animate: animate });
          }
        }

        function moveTo(latitude, longitude, animate, zoomIn) {
          const zoom = zoomIn
            ? Math.min(INITIAL.maxZoom, Math.max(map.getZoom(), 14))
            : map.getZoom();
          const point = map
            .project([latitude, longitude], zoom)
            .subtract(paddingOffset());
          map.setView(map.unproject(point, zoom), zoom, {
            animate: Boolean(animate),
          });
          appliedOffset = paddingOffset();
        }

        function countText(item) {
          if (item.capacity == null) {
            return String(item.signedCount);
          }
          return item.signedCount + "/" + item.capacity;
        }

        function peopleText(item) {
          if (item.capacity == null) {
            return item.signedCount + " people";
          }
          return item.signedCount + " of " + item.capacity + " people";
        }

        function markerLabel(item) {
          const place = item.placeName ? ", " + item.placeName : "";
          return (
            item.title +
            ", " +
            item.startsAtLabel +
            place +
            ", " +
            peopleText(item)
          );
        }

        function postViewport() {
          const center = map.getCenter();
          const bounds = map.getBounds();
          post({
            type: "viewport",
            latitude: center.lat,
            longitude: center.lng,
            zoom: map.getZoom(),
            bbox:
              bounds.getWest() +
              "," +
              bounds.getSouth() +
              "," +
              bounds.getEast() +
              "," +
              bounds.getNorth(),
          });
        }

        function setSelected(id) {
          selectedId = id;
          markerLayer.eachLayer((marker) => {
            const selected = marker.eventId === id;
            const element = marker.getElement();
            const root = element && element.querySelector(".event-marker");
            if (root) {
              root.classList.toggle("is-selected", selected);
            }
            if (element) {
              element.setAttribute("aria-expanded", selected ? "true" : "false");
            }
            marker.setZIndexOffset(selected ? 1000 : 0);
          });
        }

        function setMarkers(markers) {
          markerLayer.clearLayers();

          markers.forEach((item) => {
            const selected = item.id === selectedId;
            const label = markerLabel(item);
            const root = document.createElement("div");
            root.className = "event-marker" + (selected ? " is-selected" : "");

            const card = document.createElement("div");
            card.className = "event-card";
            card.setAttribute("aria-hidden", "true");

            const title = document.createElement("p");
            title.className = "event-title";
            title.textContent = item.title;

            const time = document.createElement("p");
            time.className = "event-time";
            time.textContent = item.startsAtLabel;

            card.append(title, time);

            if (item.placeName) {
              const place = document.createElement("p");
              place.className = "event-place";
              place.textContent = item.placeName;
              card.append(place);
            }

            const count = document.createElement("p");
            count.className = "event-count";
            const countIcon = document.createElement("span");
            countIcon.innerHTML = USERS_ICON;
            const countLabel = document.createElement("span");
            countLabel.textContent = countText(item);
            count.append(countIcon, countLabel);
            card.append(count);

            const bubble = document.createElement("div");
            bubble.className = "event-bubble";
            bubble.setAttribute("aria-hidden", "true");
            bubble.textContent = item.emoji;

            root.append(card, bubble);

            const icon = L.divIcon({
              className: "pin-wrap",
              html: root,
              iconSize: [48, 48],
              iconAnchor: [24, 24],
            });

            const marker = L.marker([item.latitude, item.longitude], {
              icon: icon,
              title: label,
              keyboard: true,
              alt: label,
              zIndexOffset: selected ? 1000 : 0,
            });
            marker.eventId = item.id;
            marker.on("add", () => {
              const element = marker.getElement();
              if (!element) {
                return;
              }
              element.setAttribute("aria-label", label);
              element.setAttribute("aria-expanded", selected ? "true" : "false");
            });
            marker.on("click", (event) => {
              suppressMapClick = true;
              const target = event.originalEvent && event.originalEvent.target;
              if (
                target &&
                typeof target.closest === "function" &&
                target.closest(".event-card")
              ) {
                return;
              }
              post({ type: "marker-press", id: item.id });
            });
            marker.addTo(markerLayer);
          });
        }

        window.__styrtaMap = {
          setPadding(next) {
            padding = next;
            applyPadding(false);
          },
          resize(animate) {
            const options = { animate: Boolean(animate), pan: true };
            map.invalidateSize(options);
            setTimeout(function () {
              map.invalidateSize({ animate: false, pan: true });
            }, 50);
          },
          zoomBy(delta, animate) {
            const current = map.getZoom();
            const next = Math.min(
              INITIAL.maxZoom,
              Math.max(INITIAL.minZoom, current + delta),
            );
            if (Math.abs(next - current) < 0.01) {
              post({
                type: "zoom-blocked",
                direction: delta > 0 ? "in" : "out",
              });
              return;
            }
            zoomSource = "control";
            map.setZoom(next, { animate: Boolean(animate) });
          },
          setMarkers: setMarkers,
          setSelected: setSelected,
          centerOn(latitude, longitude, animate) {
            moveTo(latitude, longitude, animate, true);
          },
          panTo(latitude, longitude, animate) {
            moveTo(latitude, longitude, animate, false);
          },
          setUserLocation(latitude, longitude) {
            const latLng = [latitude, longitude];
            if (!userMarker) {
              const dot = document.createElement("div");
              dot.className = "user-location";
              userMarker = L.marker(latLng, {
                icon: L.divIcon({
                  className: "pin-wrap",
                  html: dot,
                  iconSize: [34, 34],
                  iconAnchor: [17, 17],
                }),
                keyboard: false,
                interactive: false,
                title: "Your location",
                alt: "Your location",
                zIndexOffset: 500,
              }).addTo(map);
            } else {
              userMarker.setLatLng(latLng);
            }
          },
        };

        markerLayer.addTo(map);

        map.on("moveend", () => {
          postViewport();
        });

        map.on("zoomend", () => {
          post({
            type: "zoom",
            zoom: map.getZoom(),
            source: zoomSource,
          });
          zoomSource = "gesture";
          postViewport();
        });

        map.on("click", () => {
          if (suppressMapClick) {
            suppressMapClick = false;
            return;
          }
          post({ type: "map-press" });
        });

        map.whenReady(() => {
          post({ type: "ready", zoom: map.getZoom() });
          postViewport();
        });
      })();
    </script>
  </body>
</html>`;
}
