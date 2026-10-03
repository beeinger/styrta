export type MapMarker = {
  id: string;
  latitude: number;
  longitude: number;
  title: string;
  emoji: string;
  hostName: string;
  signedCount: number;
  capacity: number;
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
        background: #f2f2f7;
      }
      #map {
        touch-action: none;
      }
      .leaflet-container {
        background: #f2f2f7;
        font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
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
        border-radius: 24px;
        background: #ffffff;
        border: 2px solid #1c1c1e;
        box-shadow: 0 2px 6px rgba(0, 0, 0, 0.22);
        display: flex;
        align-items: center;
        justify-content: center;
        font-size: 24px;
        line-height: 1;
      }
      .event-marker.is-selected .event-bubble {
        border-color: #0a84ff;
      }
      .event-card {
        display: none;
        position: absolute;
        left: 50%;
        bottom: calc(100% + 10px);
        transform: translateX(-50%);
        width: max-content;
        max-width: 200px;
        background: #ffffff;
        border: 2px solid #1c1c1e;
        border-radius: 16px;
        padding: 10px 14px 12px;
        text-align: center;
        pointer-events: none;
      }
      .event-marker.is-selected .event-card {
        display: block;
      }
      .event-card-tail {
        position: absolute;
        left: 50%;
        bottom: -7px;
        width: 12px;
        height: 12px;
        margin-left: -6px;
        background: #ffffff;
        border-right: 2px solid #1c1c1e;
        border-bottom: 2px solid #1c1c1e;
        transform: rotate(45deg);
      }
      .event-host,
      .event-title,
      .event-time,
      .event-count {
        margin: 0;
        overflow-wrap: anywhere;
      }
      .event-host {
        color: #636366;
        font-size: 13px;
        line-height: 18px;
      }
      .event-title {
        margin-top: 2px;
        color: #1c1c1e;
        font-size: 16px;
        font-weight: 600;
        line-height: 22px;
      }
      .event-time {
        margin-top: 2px;
        color: #636366;
        font-size: 13px;
        line-height: 18px;
      }
      .event-count {
        margin-top: 4px;
        color: #1c1c1e;
        font-size: 15px;
        font-weight: 600;
        line-height: 20px;
      }
      .user-location {
        width: 18px;
        height: 18px;
        border-radius: 9px;
        background: #0a84ff;
        border: 3px solid #ffffff;
        box-shadow: 0 0 0 8px rgba(10, 132, 255, 0.25);
      }
    </style>
  </head>
  <body>
    <div id="map"></div>
    <script>
      const INITIAL = ${config};

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

        function markerLabel(item) {
          return (
            item.title +
            ", " +
            item.startsAtLabel +
            ", hosted by " +
            item.hostName +
            ", " +
            item.signedCount +
            " of " +
            item.capacity +
            " people"
          );
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

            const host = document.createElement("p");
            host.className = "event-host";
            host.textContent = item.hostName;

            const title = document.createElement("p");
            title.className = "event-title";
            title.textContent = item.title;

            const time = document.createElement("p");
            time.className = "event-time";
            time.textContent = item.startsAtLabel;

            const count = document.createElement("p");
            count.className = "event-count";
            count.textContent = item.signedCount + "/" + item.capacity;

            const tail = document.createElement("div");
            tail.className = "event-card-tail";
            tail.setAttribute("aria-hidden", "true");

            card.append(host, title, time, count, tail);

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
            marker.on("click", () => {
              suppressMapClick = true;
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
          setUserLocation(latitude, longitude) {
            const latLng = [latitude, longitude];
            if (!userMarker) {
              const dot = document.createElement("div");
              dot.className = "user-location";
              userMarker = L.marker(latLng, {
                icon: L.divIcon({
                  className: "pin-wrap",
                  html: dot,
                  iconSize: [18, 18],
                  iconAnchor: [9, 9],
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

        map.on("zoomend", () => {
          post({
            type: "zoom",
            zoom: map.getZoom(),
            source: zoomSource,
          });
          zoomSource = "gesture";
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
        });
      })();
    </script>
  </body>
</html>`;
}
