;;; mag-load-live-exwm.el --- Load Mag startup into a live Medley EXWM window.

(require 'exwm nil t)
(require 'exwm-input nil t)

(defun mag-medley-live--target ()
  "Return (ID BUFFER) for the first live Medley EXWM buffer."
  (let (result)
    (when (boundp 'exwm--id-buffer-alist)
      (dolist (cell exwm--id-buffer-alist)
        (let ((id (car cell))
              (buffer (cdr cell)))
          (when (and id (buffer-live-p buffer))
            (with-current-buffer buffer
              (when (and (derived-mode-p 'exwm-mode)
                         (or (and (boundp 'exwm-title)
                                  (stringp exwm-title)
                                  (string-match-p "Medley Interlisp" exwm-title))
                             (and (boundp 'exwm-class-name)
                                  (stringp exwm-class-name)
                                  (member (downcase exwm-class-name)
                                          '("lde" "ldex" "maiko")))))
                (setq result (list id buffer))))))))
    result))

(defun mag-medley-live--fake-string (text)
  "Send TEXT as synthetic key events to the selected EXWM client."
  (dolist (ch (string-to-list text))
    (exwm-input--fake-key ch)
    (sit-for 0.006)))

(defun mag-medley-load-live-exwm ()
  "Type a Mag startup LOAD expression into the live Medley XCL exec."
  (let ((target (mag-medley-live--target))
        (form "(IL::LOAD \"/home/mag/src/medley/greetfiles/MAG-NOGREET\" T)"))
    (unless target
      (error "No live Medley EXWM buffer found"))
    (let ((id (car target))
          (buffer (cadr target)))
      (switch-to-buffer buffer)
      (when (fboundp 'mag/exwm-give-keyboard)
        (mag/exwm-give-keyboard id))
      (sit-for 0.2)
      ;; Submit any accidental prompt junk first; blank input is harmless.
      (exwm-input--fake-key 'return)
      (sit-for 0.2)
      (mag-medley-live--fake-string form)
      (exwm-input--fake-key 'return)
      (message "Sent Mag startup load to Medley window %s" id)
      id)))

(mag-medley-load-live-exwm)
