export type MapMarker = {
  id: string;
  latitude: number;
  longitude: number;
  title: string;
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
        background: none;
        border: none;
      }
      .pin {
        width: 28px;
        height: 28px;
        border-radius: 50% 50% 50% 0;
        background: #1c1c1e;
        border: 2px solid #ffffff;
        box-shadow: 0 2px 4px rgba(0, 0, 0, 0.28);
        transform: rotate(-45deg);
      }
      .pin-selected {
        background: #0a84ff;
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

        function setSelected(id) {
          selectedId = id;
          document.querySelectorAll(".pin").forEach((element) => {
            element.classList.toggle("pin-selected", element.dataset.id === id);
          });
        }

        function setMarkers(markers) {
          markerLayer.clearLayers();

          markers.forEach((item) => {
            const pin = document.createElement("div");
            pin.className = "pin";
            pin.dataset.id = String(item.id);
            if (item.id === selectedId) {
              pin.classList.add("pin-selected");
            }

            const icon = L.divIcon({
              className: "pin-wrap",
              html: pin,
              iconSize: [28, 36],
              iconAnchor: [14, 36],
            });

            const marker = L.marker([item.latitude, item.longitude], {
              icon: icon,
              title: item.title,
              keyboard: true,
              alt: item.title,
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
