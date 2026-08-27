;; /// conda-script
;; channels = ["conda-forge"]
;; entrypoint = "sbcl --script ${SCRIPT}"
;;
;; [dependencies]
;; sbcl = "*"
;; zlib = "*"
;; /// end-conda-script

(sb-alien:load-shared-object
 (concatenate 'string (sb-ext:posix-getenv "CONDA_PREFIX") "/lib/libz.so"))

(sb-alien:define-alien-routine "crc32" sb-alien:unsigned-long
  (crc sb-alien:unsigned-long)
  (buf sb-alien:c-string)
  (len sb-alien:unsigned-int))

(format t "crc32(hello world) = ~D~%" (crc32 0 "hello world" 11))
