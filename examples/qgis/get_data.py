import geopandas as gpd
import requests

# URL for USGS data feed for all earthquakes in the last 7 days
url = "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/all_week.geojson"

# Send HTTP request to the URL
response = requests.get(url)

# Load data to GeoDataFrame
gdf = gpd.read_file(url)

# Save to local GeoJSON file
gdf.to_file("earthquakes.geojson", driver="GeoJSON")

print("Data downloaded and saved to earthquakes.geojson")
