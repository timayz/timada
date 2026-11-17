# An order's page: its status, its lines, its parcels, the shopper's reviews
# of its products and its invoice; and the mailed review link once expired.

order_title = Order { $reference }
order_thanks = Thank you for your order!
order_recorded_before = Your order
order_recorded_after = is recorded.
order_status_cancelling = Cancellation in progress.
order_status_authorized_supplier = Recorded: your card will be charged as soon as the shop has confirmed it.
order_status_authorized = Recorded: the payment is in progress.
order_status_paid = Paid: the shop is preparing it.
order_status_fulfilled = Shipped: it is on its way.
order_status_refunded = Cancelled, and the payment refunded.
order_status_cancelled = Cancelled. Nothing was charged.
order_cancelled_notice = Your order is cancelled: nothing was charged.
order_cancel = Cancel my order
order_cancel_text = As long as the shop has not ordered it from its supplier, your order can be cancelled here, and nothing is charged.
order_cancel_confirm = Confirm the cancellation
order_tracking_number = Parcel number:
order_parcel = Parcel { $number }
order_discount_before = Promo code
order_discount_after = : -{ $amount }
order_shipping = Delivery ({ $name }): { $amount }
order_shipping_free = Free
order_total = Total:
order_delivery_estimate = Estimated delivery: { $days } after the order
order_shipping_address = Delivery address
order_billing_address = Billing address
order_confirmation_sent = A confirmation is sent to { $email }.
order_reviews_title = Your reviews
order_review_sent = Thank you! Your review has been passed on to the shop.
order_review_revised = Your changes have been passed on to the shop.
order_review_pending = Your review is waiting to be published by the shop.
order_review_published = Your review is published. Thank you!
order_review_rejected = The shop did not publish your review.
order_review_reply = Reply from the shop
order_review_edit = Edit my review
order_review_rating = Your rating
order_review_stars =
    { $count ->
        [one] { $count } star
       *[other] { $count } stars
    }
order_review_title = Title (optional)
order_review_body = Your review
order_review_kept_photos = Your photos
order_review_photo_alt = A photo you added
order_review_keep_photo = Keep this photo
order_review_add_photos = Add photos (up to { $max } in all)
order_review_save = Save my changes
order_review_photos = Photos (optional, up to { $max })
order_review_send = Send my review
order_invoice_title = Invoice
order_invoice_pending = Your invoice is being prepared: reload this page in a moment.
order_document_credit_note = Credit note { $label }
order_document_invoice = Invoice { $label }
order_review_link_expired_title = Link expired
order_review_link_expired_heading = This link has expired
order_review_link_expired_text = The link to review your order no longer works. Sign in with the e-mail address you ordered with to find your orders and give your review.
