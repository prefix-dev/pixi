import http.server
import socketserver
import urllib.request
from urllib.error import URLError, HTTPError
import base64

PORT = 8000
PYPI_URL = 'https://pypi.org/simple'

class ProxyHTTPRequestHandler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        # Check for basic authentication
        if self.headers.get('Authorization') is None:
            self.send_response(401)
            self.send_header('WWW-Authenticate', 'Basic realm="PyPI Proxy"')
            self.end_headers()
            self.wfile.write(b'Authentication required')
            return

        # Decode the authentication token
        auth = self.headers['Authorization']
        if not auth.startswith('Basic '):
            self.send_error(401, 'Unauthorized')
            return

        auth_decoded = base64.b64decode(auth[6:]).decode('utf-8')
        username, _, password = auth_decoded.partition(':')

        # Here you can implement your check for username and password
        if username != 'admin' or password != 'password':
            self.send_error(403, 'Forbidden')
            return

        # Proxy the request to PyPI
        try:
            url = PYPI_URL + self.path
            req = urllib.request.Request(url)
            with urllib.request.urlopen(req) as response:
                self.send_response(response.status)
                for header, value in response.getheaders():
                    self.send_header(header, value)
                self.end_headers()
                self.wfile.write(response.read())
        except HTTPError as e:
            self.send_error(e.code, e.reason)
        except URLError as e:
            self.send_error(500, str(e.reason))

if __name__ == '__main__':
    with socketserver.TCPServer(("", PORT), ProxyHTTPRequestHandler) as httpd:
        print(f"Serving at port {PORT}")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            httpd.shutdown()
