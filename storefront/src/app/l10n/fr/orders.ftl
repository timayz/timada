# An order's page: its status, its lines, its parcels, the shopper's reviews
# of its products and its invoice; and the mailed review link once expired.

order_title = Commande { $reference }
order_thanks = Merci pour votre commande !
order_recorded_before = Votre commande
order_recorded_after = est enregistrée.
order_status_cancelling = Annulation en cours.
order_status_authorized_supplier = Enregistrée : votre carte sera débitée dès que la boutique l'aura confirmée.
order_status_authorized = Enregistrée : le paiement est en cours.
order_status_paid = Payée : la boutique la prépare.
order_status_fulfilled = Expédiée : elle est en route.
order_status_refunded = Annulée, et le paiement remboursé.
order_status_cancelled = Annulée. Rien n'a été débité.
order_cancelled_notice = Votre commande est annulée : rien n'a été débité.
order_cancel = Annuler ma commande
order_cancel_text = Tant que la boutique ne l'a pas commandée à son fournisseur, votre commande s'annule ici, et rien n'est débité.
order_cancel_confirm = Confirmer l'annulation
order_tracking_number = Numéro de colis :
order_parcel = Colis { $number }
order_discount_before = Code promo
order_discount_after = {" "}: -{ $amount }
order_shipping = Livraison ({ $name }) : { $amount }
order_shipping_free = Offerte
order_total = Total :
order_delivery_estimate = Livraison estimée : { $days } après la commande
order_shipping_address = Adresse de livraison
order_billing_address = Adresse de facturation
order_confirmation_sent = Une confirmation est envoyée à { $email }.
order_reviews_title = Vos avis
order_review_sent = Merci ! Votre avis a été transmis à la boutique.
order_review_revised = Vos modifications ont été transmises à la boutique.
order_review_pending = Votre avis est en attente de publication par la boutique.
order_review_published = Votre avis est publié. Merci !
order_review_rejected = La boutique n'a pas publié votre avis.
order_review_reply = Réponse de la boutique
order_review_edit = Modifier mon avis
order_review_rating = Votre note
order_review_stars =
    { $count ->
        [one] { $count } étoile
       *[other] { $count } étoiles
    }
order_review_title = Titre (facultatif)
order_review_body = Votre avis
order_review_kept_photos = Vos photos
order_review_photo_alt = Une photo que vous avez ajoutée
order_review_keep_photo = Conserver cette photo
order_review_add_photos = Ajouter des photos (jusqu'à { $max } en tout)
order_review_save = Enregistrer mes modifications
order_review_photos = Photos (facultatif, jusqu'à { $max })
order_review_send = Envoyer mon avis
order_invoice_title = Facture
order_invoice_pending = Votre facture est en préparation : rechargez cette page dans un instant.
order_document_credit_note = Avoir { $label }
order_document_invoice = Facture { $label }
order_review_link_expired_title = Lien expiré
order_review_link_expired_heading = Ce lien a expiré
order_review_link_expired_text = Le lien pour donner votre avis sur votre commande ne fonctionne plus. Connectez-vous avec l'adresse e-mail utilisée pour commander afin de retrouver vos commandes et donner votre avis.
