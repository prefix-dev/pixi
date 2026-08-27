// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "groovy ${SCRIPT}"
//
// [dependencies]
// groovy = "*"
// /// end-conda-script

@Grab('org.apache.commons:commons-lang3:3.18.0')
import org.apache.commons.lang3.StringUtils

println(StringUtils.capitalize('conda') + ' ' + StringUtils.reverse('tpircs'))
