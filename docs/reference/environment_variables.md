
# Environment Variables

Pixi can also be configured via environment variables.

<table>
  <thead>
    <tr>
      <th>Name</th>
      <th>Description</th>
      <th>Default</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td><code>PIXI_HOME</code></td>
      <td>Defines the directory where pixi puts its global data.</td>
      <td><a href="https://docs.rs/dirs/latest/dirs/fn.home_dir.html">HOME</a>/.pixi</td>
    </tr>
    <tr>
      <td><code>PIXI_CACHE_DIR</code></td>
      <td>Defines the directory where pixi puts its cache.</td>
      <td>
        <ul>
          <li>If <code>PIXI_CACHE_DIR</code> is not set, the <code>RATTLER_CACHE_DIR</code> environment variable is used.</li>
          <li>If that is not set, <code>XDG_CACHE_HOME/pixi</code> is used when the directory exists.</li>
          <li>If that is not set, the default cache directory of <a href="https://docs.rs/rattler/latest/rattler/fn.default_cache_dir.html">rattler::default_cache_dir</a> is used.</li>
        </ul>
      </td>
    </tr>
  </tbody>
</table>
