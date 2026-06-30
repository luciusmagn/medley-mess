(use-modules (shepherd service))

(register-services
 (list
  (service
   '(tedit-doc-sync)
   #:start (make-forkexec-constructor
            '("/home/mag/.local/bin/tedit-doc-sync")
            #:log-file "/home/mag/.local/var/log/tedit-doc-sync.log")
   #:stop (make-kill-destructor)
   #:respawn? #t)))
