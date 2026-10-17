//! What the shop says to its shoppers, in French: the library's English
//! table with every entry overridden.

use timada_storefront::{Messages, SIGNED_OUT_ELSEWHERE};

/// The countries the shop ships to, as the checkout lists them.
const COUNTRIES: &[(&str, &str)] = &[
    ("AT", "Autriche"),
    ("BE", "Belgique"),
    ("BG", "Bulgarie"),
    ("HR", "Croatie"),
    ("CY", "Chypre"),
    ("CZ", "Tchéquie"),
    ("DK", "Danemark"),
    ("EE", "Estonie"),
    ("FI", "Finlande"),
    ("FR", "France"),
    ("DE", "Allemagne"),
    ("GR", "Grèce"),
    ("HU", "Hongrie"),
    ("IE", "Irlande"),
    ("IT", "Italie"),
    ("LV", "Lettonie"),
    ("LT", "Lituanie"),
    ("LU", "Luxembourg"),
    ("MT", "Malte"),
    ("MC", "Monaco"),
    ("NL", "Pays-Bas"),
    ("PL", "Pologne"),
    ("PT", "Portugal"),
    ("RO", "Roumanie"),
    ("SK", "Slovaquie"),
    ("SI", "Slovénie"),
    ("ES", "Espagne"),
    ("SE", "Suède"),
    ("CA", "Canada"),
    ("NO", "Norvège"),
    ("CH", "Suisse"),
    ("GB", "Royaume-Uni"),
    ("US", "États-Unis"),
];

/// How a refusal reads to a shopper.
const REFUSALS: &[(&str, &str)] = &[
    ("invalid_quantity", "Choisissez une quantité de 1 à 99."),
    (
        "too_many_lines",
        "Votre panier est plein : commandez d'abord ce qu'il contient.",
    ),
    (
        "product_unavailable",
        "Désolé, ce produit n'est plus en vente.",
    ),
    ("variant_required", "Choisissez d'abord une option."),
    ("variant_not_found", "Cette combinaison n'est pas en vente."),
    (
        "mixed_currencies",
        "Ce produit est vendu dans une autre devise que votre panier.",
    ),
    ("empty_cart", "Votre panier est vide."),
    (
        "price_changed",
        "Un prix a changé depuis votre visite : vérifiez le nouveau total et confirmez à nouveau.",
    ),
    (
        "payments_unavailable",
        "Le paiement est indisponible pour le moment. Réessayez plus tard.",
    ),
    (
        "invoicing_not_configured",
        "Cette boutique ne peut pas encore prendre de commandes : sa configuration n'est pas terminée. Réessayez plus tard.",
    ),
    (
        "payment_failed",
        "Le paiement n'a pas abouti. Réessayez ou utilisez une autre carte.",
    ),
    (
        "payment_expired",
        "Ce paiement n'est plus ouvert. Veuillez recommencer votre commande.",
    ),
    (
        "payment_not_found",
        "Ce paiement n'est plus ouvert. Veuillez recommencer votre commande.",
    ),
    ("invalid_email", "Saisissez une adresse e-mail valide."),
    (
        "invalid_vat_number",
        "Saisissez un numéro de TVA intracommunautaire : le préfixe du pays, puis lettres et chiffres.",
    ),
    (
        "shipping_unavailable",
        "Choisissez un mode de livraison qui dessert votre pays.",
    ),
    (
        "business_buyers_refused",
        "Cette boutique ne vend pas aux professionnels : laissez le numéro de TVA vide.",
    ),
    ("invalid_ship_name", "Saisissez le nom du destinataire."),
    (
        "invalid_address",
        "Saisissez la rue, le code postal et la ville.",
    ),
    (
        "invalid_country",
        "Saisissez le pays sous forme de code à deux lettres, par exemple FR.",
    ),
    ("invalid_phone", "Le numéro de téléphone est trop long."),
    ("invalid_note", "La remarque est trop longue."),
    ("cart_not_found", "Ce panier a déjà été commandé."),
    ("cart_checked_out", "Ce panier a déjà été commandé."),
    ("discount_unknown", "Ce code promo n'est pas valable."),
    (
        "discount_inactive",
        "Ce code promo ne peut pas être utilisé pour le moment.",
    ),
    (
        "discount_currency",
        "Ce code promo concerne une autre devise.",
    ),
    (
        "discount_exhausted",
        "Ce code promo a été utilisé autant de fois que possible.",
    ),
    (
        "discount_already_used",
        "Vous avez déjà utilisé ce code promo.",
    ),
    (
        "offer_ended",
        "Cette offre n'est plus disponible : l'article a été ajouté à son prix habituel.",
    ),
    ("review_invalid_rating", "Choisissez de une à cinq étoiles."),
    (
        "review_invalid_title",
        "Le titre fait au plus 100 caractères.",
    ),
    (
        "review_invalid_body",
        "Dites-nous en un peu plus : de 10 à 2000 caractères.",
    ),
    ("review_too_many_photos", "Joignez au plus trois photos."),
    (
        "review_invalid_photo",
        "Joignez des photos JPEG, PNG, WebP ou GIF de 8 Mio au plus.",
    ),
    (
        "review_order_not_found",
        "Cette commande ne peut pas être évaluée.",
    ),
    (
        "review_not_purchased",
        "Ce produit ne fait pas partie de cette commande.",
    ),
    (
        "review_order_cancelled",
        "Une commande annulée ne peut pas être évaluée.",
    ),
    (
        "review_already_reviewed",
        "Vous avez déjà donné votre avis sur ce produit.",
    ),
    (
        "review_not_pending",
        "La boutique a déjà examiné votre avis : il ne peut plus être modifié.",
    ),
    ("review_not_found", "Cet avis est introuvable."),
    (
        "review_photo_not_found",
        "L'une de vos photos a été retirée entre-temps : réessayez.",
    ),
    (
        "wrong_code",
        "Ce n'est pas le code que nous avons envoyé. Vérifiez l'e-mail et réessayez.",
    ),
    ("code_expired", "Ce code a expiré : demandez-en un nouveau."),
    (
        "too_many_attempts",
        "Trop de codes erronés : demandez-en un nouveau.",
    ),
    (
        "code_recently_sent",
        "Un code a été envoyé il y a moins d'une minute : vérifiez votre boîte de réception.",
    ),
    (
        "too_many_requests",
        "Trop de codes demandés : réessayez dans quelques minutes.",
    ),
    (
        "too_many_addresses",
        "Votre compte conserve dix adresses : retirez-en une d'abord.",
    ),
    (
        "signed_out",
        "Vous avez été déconnecté. Veuillez vous reconnecter.",
    ),
    (
        "writes_paused",
        "Cette boutique n'accepte temporairement aucune modification. Réessayez dans quelques minutes.",
    ),
];

pub fn french() -> Messages {
    Messages::english()
        .codes(REFUSALS.iter().copied())
        .fallback("Une erreur s'est produite. Veuillez réessayer.")
        .done(
            SIGNED_OUT_ELSEWHERE,
            "Vous êtes déconnecté de tous les autres navigateurs.",
        )
        .order_statuses("Payée", "Expédiée", "Annulée")
        .review_statuses(
            "Votre avis est en attente de publication par la boutique.",
            "Votre avis est publié. Merci !",
            "La boutique n'a pas publié votre avis.",
        )
        .countries(COUNTRIES.iter().copied())
        .default_country("FR")
        .shipping_free("Offerte")
        .shipping_unavailable("—")
        .shipping_included("Offerte")
        .vat_prefix("dont")
        .off_sale_product("Un produit qui n'est plus vendu")
        .standard_shipping("Livraison standard")
        .something_went_wrong("Une erreur est survenue. Veuillez réessayer.")
}
